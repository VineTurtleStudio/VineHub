import { Layer, ToolType } from '../state/document';

export function hexToRgba(hex: string, opacity = 1): [number, number, number, number] {
  const clean = hex.replace('#', '');
  const full = clean.length === 3
    ? clean.split('').map(c => c + c).join('')
    : clean;
  const num = parseInt(full, 16);
  return [(num >> 16) & 255, (num >> 8) & 255, num & 255, Math.round(255 * opacity)];
}

export function rgbaToHex(r: number, g: number, b: number, a: number): string {
  const toHex = (v: number) => v.toString(16).padStart(2, '0');
  return `#${toHex(r)}${toHex(g)}${toHex(b)}${a === 255 ? '' : toHex(a)}`;
}

export interface DrawCommand {
  x: number;
  y: number;
  rgba: [number, number, number, number];
}

export class StrokeBuffer {
  private pixels = new Map<string, [number, number, number, number]>();

  set(x: number, y: number, rgba: [number, number, number, number]): void {
    const key = `${x},${y}`;
    this.pixels.set(key, rgba);
  }

  applyTo(layer: Layer): void {
    for (const [key, rgba] of this.pixels) {
      const [x, y] = key.split(',').map(Number);
      const pixelIdx = (y * (layer.pixels.length / 4)) + x;
      const offset = pixelIdx * 4;
      layer.pixels[offset] = rgba[0];
      layer.pixels[offset + 1] = rgba[1];
      layer.pixels[offset + 2] = rgba[2];
      layer.pixels[offset + 3] = rgba[3];
    }
  }

  clear(): void {
    this.pixels.clear();
  }

  get size(): number {
    return this.pixels.size;
  }
}

function setPixel(
  pixels: Uint8Array,
  width: number,
  height: number,
  x: number,
  y: number,
  rgba: [number, number, number, number],
): void {
  if (x < 0 || y < 0 || x >= width || y >= height) return;
  const offset = (y * width + x) * 4;
  pixels[offset] = rgba[0];
  pixels[offset + 1] = rgba[1];
  pixels[offset + 2] = rgba[2];
  pixels[offset + 3] = rgba[3];
}

export function drawBrush(
  layer: Layer,
  width: number,
  height: number,
  x: number,
  y: number,
  size: number,
  color: string,
  erasing: boolean,
  opacity = 1,
): void {
  const rgba: [number, number, number, number] = erasing
    ? [0, 0, 0, 0]
    : hexToRgba(color, opacity);
  const radius = Math.floor(size / 2);

  for (let dy = -radius; dy <= radius; dy++) {
    for (let dx = -radius; dx <= radius; dx++) {
      if (dx * dx + dy * dy > radius * radius) continue;
      setPixel(layer.pixels, width, height, x + dx, y + dy, rgba);
    }
  }
}

export function fillRegion(
  layer: Layer,
  width: number,
  height: number,
  startX: number,
  startY: number,
  color: string,
  opacity = 1,
  tolerance = 0,
): void {
  const rgba = hexToRgba(color, opacity);
  const idx = (startY * width + startX) * 4;
  const target = [
    layer.pixels[idx],
    layer.pixels[idx + 1],
    layer.pixels[idx + 2],
    layer.pixels[idx + 3],
  ];

  if (tolerance <= 0 && rgba.every((v, i) => v === target[i])) return;

  const threshold = tolerance * tolerance * 4;
  const stack: [number, number][] = [[startX, startY]];
  const visited = new Set<number>([]);

  while (stack.length > 0) {
    const [x, y] = stack.pop()!;
    if (x < 0 || y < 0 || x >= width || y >= height) continue;

    const loc = y * width + x;
    if (visited.has(loc)) continue;
    visited.add(loc);

    const i = loc * 4;
    const dr = layer.pixels[i] - target[0];
    const dg = layer.pixels[i + 1] - target[1];
    const db = layer.pixels[i + 2] - target[2];
    const da = layer.pixels[i + 3] - target[3];
    const dist2 = dr * dr + dg * dg + db * db + da * da;
    if (tolerance > 0 ? dist2 > threshold : dist2 !== 0) continue;

    layer.pixels[i] = rgba[0];
    layer.pixels[i + 1] = rgba[1];
    layer.pixels[i + 2] = rgba[2];
    layer.pixels[i + 3] = rgba[3];

    stack.push([x + 1, y]);
    stack.push([x - 1, y]);
    stack.push([x, y + 1]);
    stack.push([x, y - 1]);
  }
}

export function drawLine(
  layer: Layer,
  width: number,
  height: number,
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  size: number,
  color: string,
  opacity = 1,
): void {
  const rgba = hexToRgba(color, opacity);
  const dx = Math.abs(x1 - x0);
  const dy = Math.abs(y1 - y0);
  const sx = x0 < x1 ? 1 : -1;
  const sy = y0 < y1 ? 1 : -1;
  let err = dx - dy;
  const radius = Math.floor(size / 2);

  while (true) {
    for (let iy = -radius; iy <= radius; iy++) {
      for (let ix = -radius; ix <= radius; ix++) {
        if (ix * ix + iy * iy > radius * radius) continue;
        setPixel(layer.pixels, width, height, x0 + ix, y0 + iy, rgba);
      }
    }

    if (x0 === x1 && y0 === y1) break;
    const e2 = 2 * err;
    if (e2 > -dy) {
      err -= dy;
      x0 += sx;
    }
    if (e2 < dx) {
      err += dx;
      y0 += sy;
    }
  }
}

export function drawRect(
  layer: Layer,
  width: number,
  height: number,
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  size: number,
  color: string,
  filled: boolean,
  opacity = 1,
): void {
  const rgba = hexToRgba(color, opacity);
  const minX = Math.min(x0, x1);
  const maxX = Math.max(x0, x1);
  const minY = Math.min(y0, y1);
  const maxY = Math.max(y0, y1);

  if (filled) {
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        setPixel(layer.pixels, width, height, x, y, rgba);
      }
    }
    return;
  }

  const t = Math.max(1, Math.floor(size / 2));
  for (let d = 0; d < t; d++) {
    for (let x = minX; x <= maxX; x++) {
      setPixel(layer.pixels, width, height, x, minY + d, rgba);
      setPixel(layer.pixels, width, height, x, maxY - d, rgba);
    }
    for (let y = minY; y <= maxY; y++) {
      setPixel(layer.pixels, width, height, minX + d, y, rgba);
      setPixel(layer.pixels, width, height, maxX - d, y, rgba);
    }
  }
}

export function drawEllipse(
  layer: Layer,
  width: number,
  height: number,
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  size: number,
  color: string,
  filled: boolean,
  opacity = 1,
): void {
  const rgba = hexToRgba(color, opacity);
  const rx = Math.max(1, Math.abs(x1 - x0) / 2);
  const ry = Math.max(1, Math.abs(y1 - y0) / 2);
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  const minX = Math.floor(cx - rx);
  const maxX = Math.ceil(cx + rx);
  const minY = Math.floor(cy - ry);
  const maxY = Math.ceil(cy + ry);
  const border = filled ? 0 : Math.max(1, Math.floor(size / 2));
  const band = filled ? 0 : Math.max(0.25, (border + 0.5) / Math.max(rx, ry));

  for (let y = minY; y <= maxY; y++) {
    for (let x = minX; x <= maxX; x++) {
      if (x < 0 || y < 0 || x >= width || y >= height) continue;
      const nx = (x - cx) / rx;
      const ny = (y - cy) / ry;
      const t = nx * nx + ny * ny;
      if (t <= 1 || (band > 0 && t >= 1 - band && t <= 1 + band)) {
        setPixel(layer.pixels, width, height, x, y, rgba);
      }
    }
  }
}

export function pickPixel(layer: Layer, width: number, x: number, y: number): string {
  const idx = (y * width + x) * 4;
  return rgbaToHex(
    layer.pixels[idx],
    layer.pixels[idx + 1],
    layer.pixels[idx + 2],
    layer.pixels[idx + 3],
  );
}

export function applyToolAction(
  tool: ToolType,
  layer: Layer,
  width: number,
  height: number,
  start: { x: number; y: number },
  current: { x: number; y: number },
  brushSize: number,
  color: string,
  opacity = 1,
  tolerance = 0,
): void {
  switch (tool) {
    case 'brush':
      drawBrush(layer, width, height, current.x, current.y, brushSize, color, false, opacity);
      break;
    case 'eraser':
      drawBrush(layer, width, height, current.x, current.y, brushSize, '#000000', true, opacity);
      break;
    case 'fill':
      fillRegion(layer, width, height, current.x, current.y, color, opacity, tolerance);
      break;
    case 'line':
      drawLine(layer, width, height, start.x, start.y, current.x, current.y, brushSize, color, opacity);
      break;
    case 'rect':
      drawRect(layer, width, height, start.x, start.y, current.x, current.y, brushSize, color, true, opacity);
      break;
    case 'ellipse':
      drawEllipse(layer, width, height, start.x, start.y, current.x, current.y, brushSize, color, true, opacity);
      break;
    case 'eyedropper':
    case 'hotspot':
    case 'move':
      break;
  }
}

export function getPixelColorFromFrame(
  layers: Layer[],
  width: number,
  x: number,
  y: number,
): string {
  const rgba: [number, number, number, number] = [0, 0, 0, 0];

  for (let li = layers.length - 1; li >= 0; li--) {
    const layer = layers[li];
    if (!layer.visible) continue;
    const idx = (y * width + x) * 4;
    const a = layer.pixels[idx + 3];
    if (a === 0) continue;

    const srcA = a / 255;
    const invA = 1 - srcA;
    rgba[0] = Math.min(255, layer.pixels[idx] * srcA + rgba[0] * invA);
    rgba[1] = Math.min(255, layer.pixels[idx + 1] * srcA + rgba[1] * invA);
    rgba[2] = Math.min(255, layer.pixels[idx + 2] * srcA + rgba[2] * invA);
    rgba[3] = Math.min(255, a + rgba[3] * invA);
  }

  return rgbaToHex(rgba[0], rgba[1], rgba[2], rgba[3]);
}