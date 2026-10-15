import { invoke } from '@tauri-apps/api/core';
import { save, open } from '@tauri-apps/plugin-dialog';
import { readFile, writeFile } from '@tauri-apps/plugin-fs';
import { bridgeRequest, hubInvoke, isHubEmbedded } from './host-bridge';
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

/** `file.exportCur` / `file.exportAni` 的响应（path + 落盘字节数）。 */
export interface ExportToPathResult {
  path: string;
  bytes: number;
}

interface FileFilter {
  name: string;
  extensions: string[];
}

const CUR_FILTERS: FileFilter[] = [{ name: 'Windows 光标', extensions: ['cur'] }];
const ANI_FILTERS: FileFilter[] = [{ name: 'Windows 动画光标', extensions: ['ani'] }];
const PNG_FILTERS: FileFilter[] = [{ name: 'PNG 图片', extensions: ['png'] }];
const IMPORT_FILTERS: FileFilter[] = [
  {
    name: '光标与图片',
    extensions: ['cur', 'ani', 'png', 'gif', 'bmp', 'jpg', 'jpeg'],
  },
];

/**
 * 宿主桥接：打开文件对话框（`fs.pick` 白名单只实现 open 模式，
 * 见 docs/bridge-spec.md §2 / PanelFrame.tsx）。取消 → 空数组。
 */
async function hubPickOpenPath(filters: FileFilter[]): Promise<string[]> {
  const { paths } = await bridgeRequest<{ paths: string[] }>('fs.pick', {
    filters,
    multiple: false,
  });
  return paths ?? [];
}

/**
 * 宿主桥接：保存路径选择。当前桥接无 save 模式（fs.pick 只有 open）
 * → 明确报错。宿主补上 `{mode:'save'}` 后放开下方调用即可，
 * 各导出函数无需改动（拿到路径后一律经 hubInvoke 写入 sidecar）。
 */
async function hubPickSavePath(defaultPath: string, filters: FileFilter[]): Promise<string> {
  void defaultPath;
  void filters;
  // const { paths } = await bridgeRequest<{ paths: string[] }>('fs.pick', {
  //   mode: 'save',
  //   defaultPath,
  //   filters,
  // });
  // if (!paths || paths.length === 0) return '';
  // return paths[0];
  throw new Error(
    '宿主模式暂不支持导出/保存：桥接 fs.pick 仅提供打开文件对话框、无保存路径选择（docs/bridge-spec.md §2）。请在独立模式下操作。',
  );
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

  // 宿主模式：路径由桥接选定 → sidecar `file.exportCur` 编码并写盘
  if (isHubEmbedded()) {
    const path = await hubPickSavePath('cursor.cur', CUR_FILTERS);
    if (!path) return false;
    await hubInvoke<ExportToPathResult>('file.exportCur', 'export_cur', {
      path,
      request: payload,
    });
    return true;
  }

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

  const payload = { images };

  // 宿主模式：多尺寸 payload 直接交 sidecar 编码写盘
  if (isHubEmbedded()) {
    const path = await hubPickSavePath('cursor.cur', CUR_FILTERS);
    if (!path) return false;
    await hubInvoke<ExportToPathResult>('file.exportCur', 'export_cur', {
      path,
      request: payload,
    });
    return true;
  }

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

  // 宿主模式：路径由桥接选定 → sidecar `file.exportAni` 编码并写盘
  if (isHubEmbedded()) {
    const path = await hubPickSavePath('cursor.ani', ANI_FILTERS);
    if (!path) return false;
    await hubInvoke<ExportToPathResult>('file.exportAni', 'export_ani', {
      path,
      request: payload,
    });
    return true;
  }

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
  // PNG 由前端 canvas 编码 + 写盘；桥接白名单无 fs 写入方法 → 宿主模式明确报错
  if (isHubEmbedded()) {
    await hubPickSavePath('cursor.png', PNG_FILTERS);
    return false;
  }

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
  // 宿主模式：字节数组经 JSON 走 sidecar `import.file`（自动检测格式）
  if (isHubEmbedded()) {
    return hubInvoke<ImportResult>('import.file', 'import_file', {
      data: Array.from(bytes),
    });
  }

  return (await invoke<ImportResult>('import_file', {
    data: Array.from(bytes),
  })) as ImportResult;
}

export async function importFile(): Promise<ImportResult | null> {
  // 宿主模式：fs.pick 选路径 → sidecar `file.importPath` 读盘解码
  if (isHubEmbedded()) {
    const paths = await hubPickOpenPath(IMPORT_FILTERS);
    if (paths.length === 0) return null;
    return hubInvoke<ImportResult>('file.importPath', 'import_file', { path: paths[0] });
  }

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
  // 项目 .vpc 为任意文本：桥接无保存对话框/写入通道，且 sidecar 无对应方法
  // （等同 rpcMethod = null → 仅独立可用）
  if (isHubEmbedded()) {
    throw new Error(
      '宿主模式暂不支持保存 .vpc 项目：桥接无保存路径选择与写入通道，请在独立模式下保存。',
    );
  }

  const path = await save({
    defaultPath: 'cursor.vpc',
    filters: [{ name: 'VinePalma 光标项目', extensions: ['vpc'] }],
  });

  if (!path) return false;

  await writeFile(path as string, new TextEncoder().encode(json));
  return true;
}

export async function openProject(): Promise<string | null> {
  // .vpc 是项目 JSON，`file.importPath` 只解码光标/图片格式，无读文本方法
  // （等同 rpcMethod = null → 仅独立可用）
  if (isHubEmbedded()) {
    throw new Error(
      '宿主模式暂不支持打开 .vpc 项目：sidecar 无读取项目文件的方法，请在独立模式下打开。',
    );
  }

  const path = await open({
    multiple: false,
    filters: [{ name: 'VinePalma 光标项目', extensions: ['vpc'] }],
  });

  if (!path) return null;

  const bytes = new Uint8Array(await readFile(path as string));
  return new TextDecoder().decode(bytes);
}
