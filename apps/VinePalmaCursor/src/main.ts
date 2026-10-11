import {
  cloneLayer,
  createDocument,
  createEmptyFrame,
  createEmptyLayer,
  createId,
  deserializeDocument,
  DocumentModel,
  fitPixelsToSize,
  flipLayerPixels,
  Frame,
  mergeFramePixels,
  scalePixelsToSquare,
  serializeDocument,
  ToolType,
  translateLayerPixels,
} from './state/document';
import { UndoManager } from './state/undo';
import { CanvasRenderer } from './editor/canvas';
import { applyToolAction, getPixelColorFromFrame } from './editor/tools';
import { PreviewRenderer } from './preview/preview';
import {
  exportAni,
  exportCurWithSizes,
  exportPngToFile,
  importFile,
  importFileData,
  ImportResult,
  openProject,
  saveProject,
} from './api';

const DEFAULT_SIZE = 32;

const doc: DocumentModel = createDocument(DEFAULT_SIZE, DEFAULT_SIZE);
const undoManager = new UndoManager();

const editorCanvas = document.getElementById('editor-canvas') as HTMLCanvasElement;
const previewCanvas = document.getElementById('preview-canvas') as HTMLCanvasElement;
const renderer = new CanvasRenderer(editorCanvas);
const previewRenderer = new PreviewRenderer(previewCanvas);

renderer.setScale(8);

let isDrawing = false;
let strokeStart = { x: 0, y: 0 };
let lastPoint = { x: 0, y: 0 };

const GRID_SIZE = 16;

interface PendingStroke {
  sx: number;
  sy: number;
}

let pendingStroke: PendingStroke | null = null;

let moveStart = { x: 0, y: 0 };
let moveSnapshot: Uint8Array | null = null;

let shapeSnapshot: Uint8Array | null = null;

let showOnion = false;
let previewPlaying = false;
let previewRAF = 0;

let spaceDown = false;
let panning = false;
let panStart = { x: 0, y: 0 };
let panScroll = { x: 0, y: 0 };

let rightErasing = false;
let altPressed = false;

function currentFrame(): Frame {
  return doc.frames[doc.activeFrame];
}

function currentLayer() {
  return currentFrame().layers[doc.activeLayer];
}

function updateTitle(): void {
  document.title = `VinePalmaCursor - ${doc.width}x${doc.height}`;
}

function getBrushSize(): number {
  const el = document.getElementById('brush-size') as HTMLInputElement;
  return parseInt(el.value, 10);
}

function getOpacity(): number {
  const el = document.getElementById('opacity') as HTMLInputElement;
  return parseInt(el.value, 10) / 100;
}

function getTolerance(): number {
  const el = document.getElementById('fill-tolerance') as HTMLInputElement;
  return parseInt(el.value, 10);
}

function toolButtonEnabled(tool: ToolType): void {
  document.querySelectorAll<HTMLElement>('.tool-btn').forEach((btn) => {
    btn.classList.toggle('active', btn.dataset.tool === tool);
  });
}

function refreshPreview(): void {
  previewRenderer.setBackground(
    (document.getElementById('preview-bg') as HTMLSelectElement).value as
      | 'light'
      | 'dark'
      | 'checkerboard',
  );
  previewRenderer.setScale(
    parseInt((document.getElementById('preview-size') as HTMLSelectElement).value, 10),
  );

  if (!previewPlaying) {
    previewRenderer.renderFrames([currentFrame()], doc.width, doc.height);
  }
}

function rerender(): void {
  renderer.render(doc.frames, doc.activeFrame, doc.width, doc.height, showOnion);
  refreshPreview();
  refreshUI();
}

function updateStatusBar(x: number, y: number): void {
  const positionEl = document.getElementById('cursor-position')!;
  const infoEl = document.getElementById('document-info')!;
  positionEl.textContent = `x: ${x}, y: ${y}`;
  infoEl.textContent = `尺寸: ${doc.width} x ${doc.height} · ${doc.frames.length} 帧`;
}

function rebuildHistoryPanel(): void {
  const list = document.getElementById('history-list')!;
  list.innerHTML = '';

  const undoCount = undoManager.undoCount();

  undoManager
    .undoLabelsTopFirst()
    .forEach((label, i) => {
      const steps = undoCount - i;
      const item = document.createElement('div');
      item.className = 'history-item undo';
      item.textContent = label;
      item.title = `回到此步骤（撤销 ${steps} 步）`;
      item.addEventListener('click', () => {
        let n = steps;
        while (n-- > 0) handleUndo();
      });
      list.appendChild(item);
    });

  const current = document.createElement('div');
  current.className = 'history-item current';
  current.textContent = '● 当前状态';
  list.appendChild(current);

  undoManager.redoLabels().forEach((label, i) => {
    const steps = i + 1;
    const item = document.createElement('div');
    item.className = 'history-item redo';
    item.textContent = label;
    item.title = `重做到此步骤（${steps} 步）`;
    item.addEventListener('click', () => {
      let n = steps;
      while (n-- > 0) handleRedo();
    });
    list.appendChild(item);
  });
}

function refreshUI(): void {
  const undoBtn = document.getElementById('undo-btn') as HTMLButtonElement;
  const redoBtn = document.getElementById('redo-btn') as HTMLButtonElement;
  undoBtn.disabled = !undoManager.canUndo();
  redoBtn.disabled = !undoManager.canRedo();

  (document.getElementById('hotspot-x') as HTMLInputElement).value = String(
    currentFrame().hotspotX,
  );
  (document.getElementById('hotspot-y') as HTMLInputElement).value = String(
    currentFrame().hotspotY,
  );
  (document.getElementById('canvas-size') as HTMLInputElement).value = String(doc.width);
  (document.getElementById('frame-delay') as HTMLInputElement).value = String(
    currentFrame().delayMs,
  );
  (document.getElementById('color-picker') as HTMLInputElement).value = doc.currentColor;

  (document.getElementById('frame-del-btn') as HTMLButtonElement).disabled =
    doc.frames.length <= 1;
  (document.getElementById('layer-del-btn') as HTMLButtonElement).disabled =
    currentFrame().layers.length <= 1;

  rebuildLayerList();
  rebuildTimeline();
  rebuildHistoryPanel();
}

function pushUndo(label = '编辑'): void {
  undoManager.pushSnapshot(doc, label);
}

function applyRestoredState(restored: {
  frames: Frame[];
  activeFrame: number;
  activeLayer: number;
  currentColor: string;
  currentTool: ToolType;
}): void {
  doc.frames = restored.frames;
  doc.activeFrame = restored.activeFrame;
  doc.activeLayer = restored.activeLayer;
  doc.currentColor = restored.currentColor;
  doc.currentTool = restored.currentTool;
  toolButtonEnabled(restored.currentTool);
  rerender();
}

function handleUndo(): void {
  const restored = undoManager.undo(doc);
  if (restored) applyRestoredState(restored);
}

function handleRedo(): void {
  const restored = undoManager.redo(doc);
  if (restored) applyRestoredState(restored);
}

function stopPreview(): void {
  if (!previewPlaying) return;
  previewPlaying = false;
  cancelAnimationFrame(previewRAF);
  (document.getElementById('preview-play-btn') as HTMLButtonElement).textContent = '▶ 播放';
}

// ---------- Layer operations ----------

function addLayer(): void {
  pushUndo('新建图层');
  const frame = currentFrame();
  const layer = createEmptyLayer(
    createId('layer'),
    `图层 ${frame.layers.length + 1}`,
    doc.width * doc.height,
  );
  frame.layers.push(layer);
  doc.activeLayer = frame.layers.length - 1;
  rerender();
}

function duplicateLayer(): void {
  pushUndo('复制图层');
  const frame = currentFrame();
  const copy = cloneLayer(frame.layers[doc.activeLayer]);
  copy.id = createId('layer');
  copy.name = `${copy.name} 副本`;
  const idx = doc.activeLayer + 1;
  frame.layers.splice(idx, 0, copy);
  doc.activeLayer = idx;
  rerender();
}

function removeLayer(): void {
  const frame = currentFrame();
  if (frame.layers.length <= 1) return;
  pushUndo('删除图层');
  frame.layers.splice(doc.activeLayer, 1);
  doc.activeLayer = Math.max(0, Math.min(doc.activeLayer, frame.layers.length - 1));
  rerender();
}

function selectLayer(index: number): void {
  doc.activeLayer = index;
  rerender();
}

function toggleLayerVisibility(index: number): void {
  const layer = currentFrame().layers[index];
  layer.visible = !layer.visible;
  rerender();
}

function moveLayer(direction: 1 | -1): void {
  const frame = currentFrame();
  const index = doc.activeLayer;
  const target = index + direction;
  if (target < 0 || target >= frame.layers.length) return;

  pushUndo('移动图层');
  const [layer] = frame.layers.splice(index, 1);
  frame.layers.splice(target, 0, layer);
  doc.activeLayer = target;
  rerender();
}

function flipActiveLayer(horizontal: boolean): void {
  pushUndo('翻转图层');
  const layer = currentLayer();
  layer.pixels = flipLayerPixels(layer.pixels, doc.width, doc.height, horizontal);
  rerender();
}

function startLayerRename(index: number, nameEl: HTMLElement): void {
  const frame = currentFrame();
  const layer = frame.layers[index];
  const input = document.createElement('input');
  input.type = 'text';
  input.value = layer.name;
  input.className = 'rename-input';
  input.title = '回车确认，Esc 取消';
  nameEl.replaceWith(input);
  input.focus();
  input.select();

  const commit = (): void => {
    const value = input.value.trim();
    if (value) layer.name = value;
    rebuildLayerList();
  };

  input.addEventListener('blur', commit);
  input.addEventListener('keydown', (e) => {
    e.stopPropagation();
    if (e.key === 'Enter') {
      input.blur();
    } else if (e.key === 'Escape') {
      input.value = layer.name;
      input.blur();
    }
  });
  input.addEventListener('mousedown', (e) => e.stopPropagation());
}

function rebuildLayerList(): void {
  const list = document.getElementById('layer-list')!;
  list.innerHTML = '';
  const frame = currentFrame();

  frame.layers.forEach((layer, i) => {
    const item = document.createElement('div');
    item.className = 'layer-item';
    if (i === doc.activeLayer) item.classList.add('active');

    const eye = document.createElement('span');
    eye.className = 'eye';
    if (!layer.visible) eye.classList.add('off');
    eye.textContent = layer.visible ? '👁' : '·';
    eye.title = layer.visible ? '隐藏图层' : '显示图层';
    eye.addEventListener('click', (e) => {
      e.stopPropagation();
      toggleLayerVisibility(i);
    });

    const name = document.createElement('span');
    name.className = 'layer-name';
    name.textContent = layer.name;
    name.title = '双击重命名';
    name.addEventListener('dblclick', (e) => {
      e.stopPropagation();
      startLayerRename(i, name);
    });

    const order = document.createElement('span');
    order.className = 'layer-order';
    order.textContent = `#${frame.layers.length - i}`;

    item.appendChild(eye);
    item.appendChild(name);
    item.appendChild(order);
    item.addEventListener('click', () => selectLayer(i));
    list.appendChild(item);
  });
}

// ---------- Timeline operations ----------

function addFrame(): void {
  pushUndo('新建帧');
  const frame = createEmptyFrame(createId('frame'), doc.width, doc.height);
  doc.frames.push(frame);
  doc.activeFrame = doc.frames.length - 1;
  doc.activeLayer = 0;
  rerender();
}

function duplicateFrame(): void {
  pushUndo('复制帧');
  const src = currentFrame();
  const copy = { ...src, id: createId('frame') };
  copy.layers = src.layers.map((l) => ({ ...cloneLayer(l), id: createId('layer') }));
  const idx = doc.activeFrame + 1;
  doc.frames.splice(idx, 0, copy);
  doc.activeFrame = idx;
  rerender();
}

function removeFrame(): void {
  if (doc.frames.length <= 1) return;
  pushUndo('删除帧');
  doc.frames.splice(doc.activeFrame, 1);
  doc.activeFrame = Math.min(doc.activeFrame, doc.frames.length - 1);
  doc.activeLayer = Math.min(doc.activeLayer, currentFrame().layers.length - 1);
  rerender();
}

function selectFrame(index: number): void {
  doc.activeFrame = index;
  doc.activeLayer = Math.min(doc.activeLayer, currentFrame().layers.length - 1);
  rerender();
}

function moveFrame(from: number, to: number): void {
  if (to < 0 || to >= doc.frames.length || from === to) return;
  pushUndo('移动帧');
  const tmp = doc.frames[from];
  doc.frames[from] = doc.frames[to];
  doc.frames[to] = tmp;
  doc.activeFrame = to;
  rerender();
}

function onFrameDelayChange(): void {
  const input = document.getElementById('frame-delay') as HTMLInputElement;
  const v = parseInt(input.value, 10);
  if (Number.isNaN(v)) return;
  currentFrame().delayMs = Math.max(0, Math.min(10000, v));
  rerender();
}

function renderThumb(canvas: HTMLCanvasElement, frame: Frame): void {
  const size = 26;
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext('2d')!;
  ctx.imageSmoothingEnabled = false;
  ctx.clearRect(0, 0, size, size);

  const merged = mergeFramePixels(frame, doc.width, doc.height);
  const off = document.createElement('canvas');
  off.width = doc.width;
  off.height = doc.height;
  const octx = off.getContext('2d')!;
  const imgData = octx.createImageData(doc.width, doc.height);
  imgData.data.set(merged);
  octx.putImageData(imgData, 0, 0);

  const ratio = Math.min(size / doc.width, size / doc.height);
  const dw = Math.round(doc.width * ratio);
  const dh = Math.round(doc.height * ratio);
  ctx.drawImage(off, Math.round((size - dw) / 2), Math.round((size - dh) / 2), dw, dh);
}

function rebuildTimeline(): void {
  const list = document.getElementById('timeline-list')!;
  list.innerHTML = '';

  doc.frames.forEach((frame, i) => {
    const item = document.createElement('div');
    item.className = 'timeline-item';
    if (i === doc.activeFrame) item.classList.add('active');

    const thumb = document.createElement('canvas');
    thumb.className = 'timeline-thumb';
    renderThumb(thumb, frame);

    const info = document.createElement('div');
    info.className = 'timeline-info';

    const name = document.createElement('span');
    name.className = 'timeline-name';
    name.textContent = `帧 ${i + 1}`;

    const delay = document.createElement('span');
    delay.className = 'timeline-delay';
    delay.textContent = `${frame.delayMs} ms`;

    info.appendChild(name);
    info.appendChild(delay);

    item.appendChild(thumb);
    item.appendChild(info);
    item.addEventListener('click', () => selectFrame(i));
    list.appendChild(item);
  });
}

// ---------- Preview playback ----------

function togglePreview(): void {
  previewPlaying = !previewPlaying;
  const btn = document.getElementById('preview-play-btn') as HTMLButtonElement;
  btn.textContent = previewPlaying ? '⏸ 暂停' : '▶ 播放';
  if (previewPlaying) {
    previewRenderer.reset();
    previewRAF = requestAnimationFrame(previewLoop);
  } else {
    cancelAnimationFrame(previewRAF);
    refreshPreview();
  }
}

function previewLoop(timestamp: number): void {
  previewRenderer.tick(doc.frames, doc.width, doc.height, timestamp);
  if (previewPlaying) {
    previewRAF = requestAnimationFrame(previewLoop);
  }
}

// ---------- Canvas editing ----------

function canvasMouseDown(e: MouseEvent): void {
  const pos = renderer.screenToImage(e.clientX, e.clientY);
  if (pos.x < 0 || pos.y < 0 || pos.x >= doc.width || pos.y >= doc.height) return;

  const frame = currentFrame();

  if (spaceDown || e.button === 1) {
    panning = true;
    panStart = { x: e.clientX, y: e.clientY };
    const wrap = document.getElementById('canvas-scroll') as HTMLElement;
    panScroll = { x: wrap.scrollLeft, y: wrap.scrollTop };
    editorCanvas.classList.add('panning', 'active');
    return;
  }

  if (altPressed && (e.button === 0 || e.button === 2)) {
    const color = getPixelColorFromFrame(frame.layers, doc.width, pos.x, pos.y);
    doc.currentColor = color;
    (document.getElementById('color-picker') as HTMLInputElement).value = color;
    refreshUI();
    return;
  }

  rightErasing = e.button === 2 && (doc.currentTool === 'brush' || doc.currentTool === 'eraser');

  if (doc.currentTool === 'hotspot') {
    frame.hotspotX = pos.x;
    frame.hotspotY = pos.y;
    rerender();
    return;
  }

  if (doc.currentTool === 'move') {
    pushUndo('移动图层');
    moveStart = { ...pos };
    moveSnapshot = new Uint8Array(currentLayer().pixels);
    return;
  }

  if (doc.currentTool === 'eyedropper') {
    const color = getPixelColorFromFrame(frame.layers, doc.width, pos.x, pos.y);
    doc.currentColor = color;
    (document.getElementById('color-picker') as HTMLInputElement).value = color;
    refreshUI();
    return;
  }

  isDrawing = true;
  strokeStart = { ...pos };
  lastPoint = { ...pos };

  if (doc.currentTool === 'fill') {
    pushUndo('填充');
    applyToolAction(
      doc.currentTool,
      currentLayer(),
      doc.width,
      doc.height,
      pos,
      pos,
      getBrushSize(),
      doc.currentColor,
      getOpacity(),
      getTolerance(),
    );
    isDrawing = false;
    rerender();
    return;
  }

  pushUndo('绘制');
  pendingStroke = { sx: pos.x, sy: pos.y };

  if (
    doc.currentTool === 'line' ||
    doc.currentTool === 'rect' ||
    doc.currentTool === 'ellipse'
  ) {
    shapeSnapshot = new Uint8Array(currentLayer().pixels);
  }

  applyToolAction(
    doc.currentTool,
    currentLayer(),
    doc.width,
    doc.height,
    strokeStart,
    pos,
    getBrushSize(),
    doc.currentColor,
    getOpacity(),
  );
  rerender();
}

function canvasMouseMove(e: MouseEvent): void {
  if (panning) {
    const wrap = document.getElementById('canvas-scroll') as HTMLElement;
    wrap.scrollLeft = panScroll.x - (e.clientX - panStart.x);
    wrap.scrollTop = panScroll.y - (e.clientY - panStart.y);
    return;
  }

  const pos = renderer.screenToImage(e.clientX, e.clientY);
  updateStatusBar(pos.x, pos.y);

  if (doc.currentTool === 'move') {
    if (!moveSnapshot) return;
    const dx = pos.x - moveStart.x;
    const dy = pos.y - moveStart.y;
    currentLayer().pixels = translateLayerPixels(
      moveSnapshot,
      doc.width,
      doc.height,
      dx,
      dy,
    );
    rerender();
    return;
  }

  if (!isDrawing) return;
  if (pos.x < 0 || pos.y < 0 || pos.x >= doc.width || pos.y >= doc.height) return;

  if (
    (doc.currentTool === 'line' ||
      doc.currentTool === 'rect' ||
      doc.currentTool === 'ellipse') &&
    shapeSnapshot
  ) {
    currentLayer().pixels.set(shapeSnapshot);
    applyToolAction(
      doc.currentTool,
      currentLayer(),
      doc.width,
      doc.height,
      strokeStart,
      pos,
      getBrushSize(),
      doc.currentColor,
      getOpacity(),
    );
  } else if (
    pendingStroke &&
    (doc.currentTool === 'brush' || doc.currentTool === 'eraser' || rightErasing)
  ) {
    const erasing = rightErasing || doc.currentTool === 'eraser';
    const tool = erasing ? 'eraser' : doc.currentTool;
    const steps = Math.max(Math.abs(pos.x - lastPoint.x), Math.abs(pos.y - lastPoint.y));
    for (let i = 0; i < steps; i++) {
      const t = (i + 1) / Math.max(steps, 1);
      const ix = Math.round(lastPoint.x + (pos.x - lastPoint.x) * t);
      const iy = Math.round(lastPoint.y + (pos.y - lastPoint.y) * t);
      applyToolAction(
        tool,
        currentLayer(),
        doc.width,
        doc.height,
        strokeStart,
        { x: ix, y: iy },
        getBrushSize(),
        doc.currentColor,
        getOpacity(),
      );
    }
    lastPoint = { ...pos };
  }

  rerender();
}

function canvasMouseUp(): void {
  if (panning) {
    panning = false;
    editorCanvas.classList.remove('active');
    return;
  }
  rightErasing = false;
  moveSnapshot = null;
  shapeSnapshot = null;
  if (!isDrawing) return;
  isDrawing = false;
  pendingStroke = null;
}

function onToolChange(tool: ToolType): void {
  doc.currentTool = tool;
  toolButtonEnabled(tool);
}

function onSizeChange(): void {
  const sizeInput = document.getElementById('canvas-size') as HTMLInputElement;
  const newSize = Math.max(16, Math.min(128, parseInt(sizeInput.value, 10) || GRID_SIZE));

  pushUndo('调整画布');
  const oldWidth = doc.width;
  const oldHeight = doc.height;
  const scaledSize = newSize;

  doc.width = scaledSize;
  doc.height = scaledSize;

  for (const frame of doc.frames) {
    for (const layer of frame.layers) {
      const newPixels = new Uint8Array(scaledSize * scaledSize * 4);
      for (let y = 0; y < Math.min(oldHeight, scaledSize); y++) {
        for (let x = 0; x < Math.min(oldWidth, scaledSize); x++) {
          const srcIdx = (y * oldWidth + x) * 4;
          const dstIdx = (y * scaledSize + x) * 4;
          newPixels[dstIdx] = layer.pixels[srcIdx];
          newPixels[dstIdx + 1] = layer.pixels[srcIdx + 1];
          newPixels[dstIdx + 2] = layer.pixels[srcIdx + 2];
          newPixels[dstIdx + 3] = layer.pixels[srcIdx + 3];
        }
      }
      layer.pixels = newPixels;
    }
    frame.hotspotX = Math.min(frame.hotspotX, scaledSize - 1);
    frame.hotspotY = Math.min(frame.hotspotY, scaledSize - 1);
  }

  rerender();
}

function onClearCanvas(): void {
  pushUndo('清空图层');
  currentLayer().pixels.fill(0);
  rerender();
}

function onHotspotChange(): void {
  const x = parseInt((document.getElementById('hotspot-x') as HTMLInputElement).value, 10);
  const y = parseInt((document.getElementById('hotspot-y') as HTMLInputElement).value, 10);
  if (Number.isNaN(x) || Number.isNaN(y)) return;
  currentFrame().hotspotX = Math.max(0, Math.min(doc.width - 1, x));
  currentFrame().hotspotY = Math.max(0, Math.min(doc.height - 1, y));
  rerender();
}

function syncHotspotToAllFrames(): void {
  pushUndo('同步热点');
  const { hotspotX, hotspotY } = currentFrame();
  for (const frame of doc.frames) {
    frame.hotspotX = hotspotX;
    frame.hotspotY = hotspotY;
  }
  rerender();
}

function fillAllFramesWithCurrent(): void {
  pushUndo('填充全部帧');
  const src = currentFrame();
  const srcLayers = src.layers.map((l) => cloneLayer(l));

  for (const frame of doc.frames) {
    frame.layers = srcLayers.map((l) => ({
      ...cloneLayer(l),
      id: createId('layer'),
    }));
    frame.hotspotX = src.hotspotX;
    frame.hotspotY = src.hotspotY;
  }
  doc.activeLayer = Math.min(doc.activeLayer, currentFrame().layers.length - 1);
  rerender();
  showModal(`已用当前帧填充全部帧 (${doc.frames.length} 帧)`);
}

async function onExportPng(): Promise<void> {
  try {
    const success = await exportPngToFile(currentFrame(), doc.width, doc.height);
    if (success) {
      showModal('PNG 导出成功！');
    }
  } catch (err) {
    console.error('PNG 导出失败:', err);
    showModal(`PNG 导出失败：${err instanceof Error ? err.message : String(err)}`);
  }
}

// ---------- Import / Export ----------

const EXPORT_SIZE_OPTIONS = [16, 24, 32, 48, 64, 96];

function renderExportPreviewGrid(
  container: HTMLElement,
  frame: Frame,
  width: number,
  height: number,
  selected: number[],
): void {
  container.innerHTML = '';

  const base = document.createElement('canvas');
  base.width = width;
  base.height = height;
  const bctx = base.getContext('2d')!;
  const imgData = bctx.createImageData(width, height);
  imgData.data.set(mergeFramePixels(frame, width, height));
  bctx.putImageData(imgData, 0, 0);

  if (selected.length === 0) {
    container.textContent = '（未勾选任何尺寸）';
    return;
  }

  for (const s of selected) {
    const unit = document.createElement('div');
    unit.className = 'export-preview-unit';

    const c = document.createElement('canvas');
    c.width = s;
    c.height = s;
    const ctx = c.getContext('2d')!;
    ctx.imageSmoothingEnabled = false;
    ctx.clearRect(0, 0, s, s);
    ctx.drawImage(base, 0, 0, s, s);

    const label = document.createElement('span');
    label.className = 'export-preview-label';
    label.textContent = `${s}px`;

    unit.appendChild(c);
    unit.appendChild(label);
    container.appendChild(unit);
  }
}

function closeModal(): void {
  document.getElementById('modal-overlay')!.classList.add('hidden');
}

function showModal(message: string): void {
  const overlay = document.getElementById('modal-overlay')!;
  const modal = document.getElementById('modal')!;
  modal.innerHTML = `
    <button class="exp-close" id="modal-close">×</button>
    <p id="modal-message"></p>
    <div class="modal-actions">
      <button id="modal-ok">确定</button>
    </div>`;
  (modal.querySelector('#modal-message') as HTMLElement).textContent = message;
  overlay.classList.remove('hidden');

  modal.querySelector('#modal-close')!.addEventListener('click', closeModal);
  modal.querySelector('#modal-ok')!.addEventListener('click', closeModal);
}

function openExportDialog(): void {
  const overlay = document.getElementById('modal-overlay')!;
  const modal = document.getElementById('modal')!;
  const frame = currentFrame();

  modal.innerHTML = `
    <button class="exp-close" id="modal-close">×</button>
    <h3>导出 .cur</h3>
    <p>选择要导出的尺寸（多选，当前帧 ${doc.width}×${doc.height}）：</p>
    <div class="export-preview-grid" id="export-preview-grid"></div>
    <div class="export-sizes-grid">
      ${EXPORT_SIZE_OPTIONS.map(
        (s) =>
          `<label><input type="checkbox" value="${s}" ${s === doc.width || s === 32 ? 'checked' : ''} />${s} px</label>`,
      ).join('')}
    </div>
    <div class="modal-actions">
      <button id="modal-cancel">取消</button>
      <button id="modal-ok" class="primary">导出</button>
    </div>`;
  overlay.classList.remove('hidden');

  const previewGrid = document.getElementById('export-preview-grid')!;
  function refreshPreview(): void {
    const checked = Array.from(
      modal.querySelectorAll<HTMLInputElement>('.export-sizes-grid input:checked'),
    ).map((el) => parseInt(el.value, 10));
    renderExportPreviewGrid(previewGrid, frame, doc.width, doc.height, checked);
  }
  refreshPreview();

  modal.querySelectorAll<HTMLInputElement>('.export-sizes-grid input').forEach((input) => {
    input.addEventListener('change', refreshPreview);
  });

  modal.querySelector('#modal-close')!.addEventListener('click', closeModal);
  modal.querySelector('#modal-cancel')!.addEventListener('click', closeModal);
  modal.querySelector('#modal-ok')!.addEventListener('click', async () => {
    closeModal();

    const selected = Array.from(
      modal.querySelectorAll<HTMLInputElement>('.export-sizes-grid input:checked'),
    ).map((el) => parseInt(el.value, 10));

    if (selected.length === 0) {
      showModal('请至少选择一个导出尺寸');
      return;
    }

    try {
      const success = await exportCurWithSizes(
        frame,
        doc.width,
        doc.height,
        selected,
        frame.hotspotX,
        frame.hotspotY,
      );
      if (success) {
        showModal('光标导出成功！');
      }
    } catch (err) {
      console.error('导出失败:', err);
      showModal(`导出失败：${err instanceof Error ? err.message : String(err)}`);
    }
  });
}

async function onExportAni(): Promise<void> {
  try {
    const success = await exportAni(doc.frames, doc.width, doc.height);
    if (success) {
      showModal(`动画光标导出成功！(${doc.frames.length} 帧)`);
    }
  } catch (err) {
    console.error('动画导出失败:', err);
    showModal(`动画导出失败：${err instanceof Error ? err.message : String(err)}`);
  }
}

function applyImportResult(result: ImportResult, targetSize?: number): void {
  const first = result.frames[0];
  const w = targetSize ?? first.width;
  const h = targetSize ?? first.height;

  const frames: Frame[] = result.frames.map((f) => {
    let rgba: Uint8Array = new Uint8Array(f.rgba);
    let srcW = f.width;
    let srcH = f.height;

    if (targetSize) {
      rgba = scalePixelsToSquare(rgba, srcW, srcH, targetSize);
      srcW = targetSize;
      srcH = targetSize;
    }

    return {
      id: createId('frame'),
      layers: [
        {
          id: createId('layer'),
          name: '图层 1',
          visible: true,
          opacity: 1,
          pixels:
            srcW === w && srcH === h
              ? rgba
              : fitPixelsToSize(rgba, srcW, srcH, w, h),
        },
      ],
      hotspotX: Math.min(f.hotspot_x, w - 1),
      hotspotY: Math.min(f.hotspot_y, h - 1),
      delayMs: f.delay_ms || 100,
    };
  });

  doc.width = w;
  doc.height = h;
  doc.frames = frames;
  doc.activeFrame = 0;
  doc.activeLayer = 0;
  doc.currentTool = 'brush';
  toolButtonEnabled('brush');

  undoManager.clear();
  stopPreview();
  updateTitle();
  rerender();
  showModal(
    `已导入 ${result.format.toUpperCase()}：${frames.length} 帧 × ${w}×${h}` +
      (targetSize && (first.width !== first.height || targetSize !== first.width)
        ? `（由 ${first.width}×${first.height} 缩放）`
        : ''),
  );
}

function askImportSize(
  width: number,
  height: number,
  allowOriginal: boolean,
): Promise<number | null> {
  return new Promise((resolve) => {
    const overlay = document.getElementById('modal-overlay')!;
    const modal = document.getElementById('modal')!;

    const originalOption = allowOriginal
      ? `<option value="0">保持原始 ${width}×${height}</option>`
      : '';
    const defaultTarget = Math.min(
      64,
      Math.max(32, Math.round(Math.min(width, height) * 2)),
    );

    modal.innerHTML = `
      <button class="exp-close" id="modal-close">×</button>
      <h3>调整图片尺寸</h3>
      <p>原图 ${width}×${height}，导入后作为光标画布（居中放置并保留透明）：</p>
      <div class="row">
        <label for="import-size-select">缩放目标</label>
        <select id="import-size-select">
          ${originalOption}
          <option value="64">64 × 64</option>
          <option value="48">48 × 48</option>
          <option value="32" ${defaultTarget === 32 ? 'selected' : ''}>32 × 32</option>
          <option value="16" ${defaultTarget === 16 ? 'selected' : ''}>16 × 16</option>
        </select>
      </div>
      <div class="modal-actions">
        <button id="modal-cancel">取消</button>
        <button id="modal-ok" class="primary">导入</button>
      </div>`;
    overlay.classList.remove('hidden');

    const select = modal.querySelector('#import-size-select') as HTMLSelectElement;
    const finish = (value: number | null): void => {
      overlay.classList.add('hidden');
      resolve(value);
    };

    modal.querySelector('#modal-close')!.addEventListener('click', () => finish(null));
    modal.querySelector('#modal-cancel')!.addEventListener('click', () => finish(null));
    modal.querySelector('#modal-ok')!.addEventListener('click', () => {
      const sel = parseInt(select.value, 10);
      finish(sel); // 0 = original
    });
  });
}

async function handleImportResult(result: ImportResult): Promise<void> {
  if (!result || result.frames.length === 0) return;

  const first = result.frames[0];
  const maxDim = Math.max(first.width, first.height);

  if (maxDim > 64 || first.width !== first.height) {
    const target = await askImportSize(first.width, first.height, maxDim <= 128);
    if (target === null) return;
    applyImportResult(result, target === 0 ? undefined : target);
  } else {
    applyImportResult(result);
  }
}

async function onImport(): Promise<void> {
  try {
    const result = await importFile();
    if (!result || result.frames.length === 0) return;
    await handleImportResult(result);
  } catch (err) {
    console.error('导入失败:', err);
    showModal(`导入失败：${err instanceof Error ? err.message : String(err)}`);
  }
}

function applyOpenProject(json: string): boolean {
  const restored = deserializeDocument(json);
  if (!restored) return false;

  doc.width = restored.width;
  doc.height = restored.height;
  doc.frames = restored.frames;
  doc.activeFrame = restored.activeFrame;
  doc.activeLayer = restored.activeLayer;
  doc.currentColor = restored.currentColor;
  doc.currentTool = restored.currentTool;
  toolButtonEnabled(restored.currentTool);

  undoManager.clear();
  stopPreview();
  updateTitle();
  rerender();
  return true;
}

async function onOpenProject(): Promise<void> {
  try {
    const json = await openProject();
    if (!json) return;
    if (applyOpenProject(json)) {
      showModal('项目已打开');
    } else {
      showModal('无法打开项目文件：格式无效');
    }
  } catch (err) {
    console.error('打开项目失败:', err);
    showModal(`打开项目失败：${err instanceof Error ? err.message : String(err)}`);
  }
}

async function onSaveProject(): Promise<void> {
  try {
    const success = await saveProject(serializeDocument(doc));
    if (success) {
      showModal('项目已保存 (.vpc)');
    }
  } catch (err) {
    console.error('保存项目失败:', err);
    showModal(`保存项目失败：${err instanceof Error ? err.message : String(err)}`);
  }
}

function setupDragDrop(): void {
  window.addEventListener('dragover', (e) => {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
  });

  window.addEventListener('drop', async (e) => {
    e.preventDefault();
    const file = e.dataTransfer?.files?.[0];
    if (!file) return;

    let bytes: Uint8Array;
    try {
      bytes = new Uint8Array(await file.arrayBuffer());
    } catch (err) {
      console.error('读取拖放文件失败:', err);
      return;
    }

    if (file.name.toLowerCase().endsWith('.vpc')) {
      const json = new TextDecoder().decode(bytes);
      if (applyOpenProject(json)) {
        showModal('项目已打开');
      } else {
        showModal('无法打开项目文件：格式无效');
      }
      return;
    }

    try {
      const result = await importFileData(bytes);
      if (result.frames.length === 0) return;
      await handleImportResult(result);
    } catch (err) {
      console.error('拖放导入失败:', err);
      showModal(`导入失败：${err instanceof Error ? err.message : String(err)}`);
    }
  });
}

// ---------- Events ----------

function setupEvents(): void {
  document.querySelectorAll<HTMLElement>('.panel-title').forEach((title) => {
    title.addEventListener('click', () => {
      const section = title.closest('.panel-section');
      if (section) section.classList.toggle('collapsed');
    });
  });

  document.querySelectorAll<HTMLElement>('.tool-btn').forEach((btn) => {
    btn.addEventListener('click', () => {
      onToolChange(btn.dataset.tool as ToolType);
    });
  });

  document.getElementById('color-picker')!.addEventListener('input', (e) => {
    doc.currentColor = (e.target as HTMLInputElement).value;
  });

  document.getElementById('undo-btn')!.addEventListener('click', handleUndo);
  document.getElementById('redo-btn')!.addEventListener('click', handleRedo);

  document.getElementById('zoom-in-btn')!.addEventListener('click', () => {
    const s = renderer.getScale();
    if (s < 32) renderer.setScale(s * 2);
    (document.getElementById('zoom-label') as HTMLElement).textContent = `${renderer.getScale()}x`;
    rerender();
  });

  document.getElementById('zoom-out-btn')!.addEventListener('click', () => {
    const s = renderer.getScale();
    if (s > 1) renderer.setScale(s / 2);
    (document.getElementById('zoom-label') as HTMLElement).textContent = `${renderer.getScale()}x`;
    rerender();
  });

  document.getElementById('canvas-scroll')!.addEventListener('wheel', (e) => {
    e.preventDefault();
    const current = renderer.getScale();
    const next = e.deltaY > 0 ? Math.max(1, current / 2) : Math.min(32, current * 2);
    if (next === current) return;
    renderer.setScale(next);
    (document.getElementById('zoom-label') as HTMLElement).textContent = `${next}x`;
    rerender();
  }, { passive: false });

  document.addEventListener('keydown', (e) => {
    const key = e.key.toLowerCase();
    if (e.ctrlKey || e.metaKey) {
      if (key === '0') {
        e.preventDefault();
        const scroll = document.getElementById('canvas-scroll') as HTMLElement;
        const fit = Math.floor(Math.min(
          scroll.clientWidth / doc.width,
          scroll.clientHeight / doc.height,
        ));
        renderer.setScale(Math.max(1, Math.min(32, fit)));
        (document.getElementById('zoom-label') as HTMLElement).textContent = `${renderer.getScale()}x`;
        rerender();
      }
    }
  });

  document.getElementById('brush-size')!.addEventListener('input', (e) => {
    (document.getElementById('brush-size-label') as HTMLElement).textContent = (
      e.target as HTMLInputElement
    ).value;
  });

  document.getElementById('opacity')!.addEventListener('input', (e) => {
    const v = (e.target as HTMLInputElement).value;
    (document.getElementById('opacity-label') as HTMLElement).textContent = `${v}%`;
  });

  document.getElementById('canvas-bg')!.addEventListener('change', (e) => {
    const v = (e.target as HTMLSelectElement).value;
    renderer.setBackground(v === 'white' ? 'white' : v === 'black' ? 'black' : 'checkerboard');
    rerender();
  });

  document.getElementById('grid-toggle')!.addEventListener('change', (e) => {
    renderer.setShowGrid((e.target as HTMLInputElement).checked);
    rerender();
  });

  document.getElementById('fill-tolerance')!.addEventListener('input', (e) => {
    (document.getElementById('fill-tolerance-label') as HTMLElement).textContent = (
      e.target as HTMLInputElement
    ).value;
  });

  document.getElementById('apply-size-btn')!.addEventListener('click', onSizeChange);
  document.getElementById('clear-canvas-btn')!.addEventListener('click', onClearCanvas);

  document.getElementById('hotspot-x')!.addEventListener('change', onHotspotChange);
  document.getElementById('hotspot-y')!.addEventListener('change', onHotspotChange);

  document.getElementById('preview-size')!.addEventListener('change', () => rerender());
  document.getElementById('preview-bg')!.addEventListener('change', () => rerender());
  document.getElementById('preview-speed')!.addEventListener('change', (e) => {
    previewRenderer.setAnimationSpeed(parseFloat((e.target as HTMLSelectElement).value));
  });
  document.getElementById('preview-play-btn')!.addEventListener('click', togglePreview);

  document.getElementById('onion-toggle')!.addEventListener('change', (e) => {
    showOnion = (e.target as HTMLInputElement).checked;
    rerender();
  });

  document.getElementById('frame-delay')!.addEventListener('change', onFrameDelayChange);

  document.getElementById('layer-add-btn')!.addEventListener('click', addLayer);
  document.getElementById('layer-dup-btn')!.addEventListener('click', duplicateLayer);
  document.getElementById('layer-del-btn')!.addEventListener('click', removeLayer);

  document.getElementById('layer-up-btn')!.addEventListener('click', () => moveLayer(1));
  document.getElementById('layer-down-btn')!.addEventListener('click', () => moveLayer(-1));
  document.getElementById('layer-flip-h-btn')!.addEventListener('click', () => flipActiveLayer(true));
  document.getElementById('layer-flip-v-btn')!.addEventListener('click', () => flipActiveLayer(false));

  document.getElementById('sync-hotspot-btn')!.addEventListener('click', syncHotspotToAllFrames);
  document.getElementById('export-png-btn')!.addEventListener('click', onExportPng);

  document.getElementById('panel-import-btn')!.addEventListener('click', onImport);
  document.getElementById('panel-export-cur-btn')!.addEventListener('click', openExportDialog);
  document.getElementById('panel-export-ani-btn')!.addEventListener('click', onExportAni);

  document.getElementById('frame-to-all-btn')!.addEventListener('click', fillAllFramesWithCurrent);

  document.getElementById('frame-add-btn')!.addEventListener('click', addFrame);
  document.getElementById('frame-dup-btn')!.addEventListener('click', duplicateFrame);
  document.getElementById('frame-del-btn')!.addEventListener('click', removeFrame);

  document.getElementById('frame-left-btn')!.addEventListener('click', () => {
    if (doc.activeFrame > 0) moveFrame(doc.activeFrame, doc.activeFrame - 1);
  });
  document.getElementById('frame-right-btn')!.addEventListener('click', () => {
    if (doc.activeFrame < doc.frames.length - 1) moveFrame(doc.activeFrame, doc.activeFrame + 1);
  });

  document.getElementById('export-btn')!.addEventListener('click', openExportDialog);
  document.getElementById('export-ani-btn')!.addEventListener('click', onExportAni);
  document.getElementById('import-btn')!.addEventListener('click', onImport);

  document.getElementById('save-project-btn')!.addEventListener('click', onSaveProject);
  document.getElementById('open-project-btn')!.addEventListener('click', onOpenProject);

  editorCanvas.addEventListener('mousedown', canvasMouseDown);
  editorCanvas.addEventListener('mousemove', canvasMouseMove);
  window.addEventListener('mouseup', canvasMouseUp);

  editorCanvas.addEventListener('contextmenu', (e) => e.preventDefault());

  document.addEventListener('keydown', (e) => {
    if (e.key === ' ') {
      if (!e.repeat && document.activeElement === document.body) {
        e.preventDefault();
        spaceDown = true;
        editorCanvas.classList.add('panning');
      }
      return;
    }
    if (e.key === 'Alt' && !e.repeat) {
      altPressed = true;
      return;
    }

    const key = e.key.toLowerCase();

    if (e.ctrlKey || e.metaKey) {
      if (key === 'z') {
        e.preventDefault();
        if (e.shiftKey) handleRedo();
        else handleUndo();
      } else if (key === 'y') {
        e.preventDefault();
        handleRedo();
      } else if (key === 's') {
        e.preventDefault();
        onSaveProject();
      } else if (key === 'o') {
        e.preventDefault();
        onOpenProject();
      }
      return;
    }

    if (e.altKey) return;

    const keyMap: Record<string, ToolType> = {
      b: 'brush',
      e: 'eraser',
      g: 'fill',
      l: 'line',
      r: 'rect',
      o: 'ellipse',
      i: 'eyedropper',
      h: 'hotspot',
      m: 'move',
    };
    if (keyMap[key]) {
      onToolChange(keyMap[key]);
    }
  });

  document.addEventListener('keyup', (e) => {
    if (e.key === ' ') {
      spaceDown = false;
      editorCanvas.classList.remove('panning');
    } else if (e.key === 'Alt') {
      altPressed = false;
    }
  });
}

function init(): void {
  updateTitle();
  renderer.setBackground('checkerboard');
  renderer.setShowGrid(true);
  previewRenderer.setAnimationSpeed(1);
  setupEvents();
  setupDragDrop();
  rerender();
}

init();