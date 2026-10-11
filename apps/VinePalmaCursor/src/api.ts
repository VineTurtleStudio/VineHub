import { invoke } from '@tauri-apps/api/core';
import { save, open } from '@tauri-apps/plugin-dialog';
import { readFile, writeFile } from '@tauri-apps/plugin-fs';
import { Frame, mergeFramePixels } from './state/document';

export interface CursorFramePayload {
  width: number;
  height: number;
  hotspot_x: number;
  hotspot_y: number;
  rgba: number[];
  delay_ms: number;
}

export interface ImportResult {
  format: string;
  frames: CursorFramePayload[];
}

function mergedRgbaArray(frame: Frame, width: number, height: number): number[] {
  return Array.from(mergeFramePixels(frame, width, height));
}

export async function exportCur(
  frame: Frame,
  width: number,
  height: number,
  hotspotX: number,
  hotspotY: number,
): Promise<boolean> {
  const payload = {
    images: [
      {
        width,
        height,
        hotspot_x: hotspotX,
        hotspot_y: hotspotY,
        rgba: mergedRgbaArray(frame, width, height),
      },
    ],
  };

  const result = (await invoke<number[]>('export_cur', { request: payload })) as number[];
  const bytes = new Uint8Array(result);

  const path = await save({
    defaultPath: 'cursor.cur',
    filters: [{ name: 'Windows 光标', extensions: ['cur'] }],
  });

  if (!path) return false;

  await writeFile(path as string, bytes);
  return true;
}

export async function exportCurWithSizes(
  frame: Frame,
  width: number,
  height: number,
  sizes: number[],
  hotspotX: number,
  hotspotY: number,
): Promise<boolean> {
  const basePixels = mergeFramePixels(frame, width, height);
  const base = document.createElement('canvas');
  base.width = width;
  base.height = height;
  const bctx = base.getContext('2d')!;
  const baseData = bctx.createImageData(width, height);
  baseData.data.set(basePixels);
  bctx.putImageData(baseData, 0, 0);

  const images: {
    width: number;
    height: number;
    hotspot_x: number;
    hotspot_y: number;
    rgba: number[];
  }[] = [];

  for (const size of sizes) {
    const scale = size / width;
    const offscreen = document.createElement('canvas');
    offscreen.width = size;
    offscreen.height = size;
    const ctx = offscreen.getContext('2d')!;

    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(base, 0, 0, size, size);

    const outData = ctx.getImageData(0, 0, size, size).data;
    images.push({
      width: size,
      height: size,
      hotspot_x: Math.round(hotspotX * scale),
      hotspot_y: Math.round(hotspotY * scale),
      rgba: Array.from(outData),
    });
  }

  const result = (await invoke<number[]>('export_cur', { request: { images } })) as number[];
  const bytes = new Uint8Array(result);

  const path = await save({
    defaultPath: 'cursor.cur',
    filters: [{ name: 'Windows 光标', extensions: ['cur'] }],
  });

  if (!path) return false;

  await writeFile(path as string, bytes);
  return true;
}

export async function exportAni(
  frames: Frame[],
  width: number,
  height: number,
): Promise<boolean> {
  const payload = {
    frames: frames.map((f) => ({
      width,
      height,
      hotspot_x: f.hotspotX,
      hotspot_y: f.hotspotY,
      rgba: mergedRgbaArray(f, width, height),
      delay_ms: f.delayMs,
    })),
  };

  const result = (await invoke<number[]>('export_ani', { request: payload })) as number[];
  const bytes = new Uint8Array(result);

  const path = await save({
    defaultPath: 'cursor.ani',
    filters: [{ name: 'Windows 动画光标', extensions: ['ani'] }],
  });

  if (!path) return false;

  await writeFile(path as string, bytes);
  return true;
}

export async function exportPngToFile(
  frame: Frame,
  width: number,
  height: number,
): Promise<boolean> {
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext('2d')!;
  const merged = mergeFramePixels(frame, width, height);
  const imgData = ctx.createImageData(width, height);
  imgData.data.set(merged);
  ctx.putImageData(imgData, 0, 0);

  const dataUrl = canvas.toDataURL('image/png');
  const base64 = dataUrl.split(',')[1];
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }

  const path = await save({
    defaultPath: 'cursor.png',
    filters: [{ name: 'PNG 图片', extensions: ['png'] }],
  });

  if (!path) return false;

  await writeFile(path as string, bytes);
  return true;
}

export async function importFileData(bytes: Uint8Array): Promise<ImportResult> {
  return (await invoke<ImportResult>('import_file', {
    data: Array.from(bytes),
  })) as ImportResult;
}

export async function importFile(): Promise<ImportResult | null> {
  const path = await open({
    multiple: false,
    filters: [
      {
        name: '光标与图片',
        extensions: ['cur', 'ani', 'png', 'gif', 'bmp', 'jpg', 'jpeg'],
      },
    ],
  });

  if (!path) return null;

  const bytes = new Uint8Array(await readFile(path as string));
  return importFileData(bytes);
}

export async function saveProject(json: string): Promise<boolean> {
  const path = await save({
    defaultPath: 'cursor.vpc',
    filters: [{ name: 'VinePalma 光标项目', extensions: ['vpc'] }],
  });

  if (!path) return false;

  await writeFile(path as string, new TextEncoder().encode(json));
  return true;
}

export async function openProject(): Promise<string | null> {
  const path = await open({
    multiple: false,
    filters: [{ name: 'VinePalma 光标项目', extensions: ['vpc'] }],
  });

  if (!path) return null;

  const bytes = new Uint8Array(await readFile(path as string));
  return new TextDecoder().decode(bytes);
}