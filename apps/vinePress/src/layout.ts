import { el } from './ui';

/** 三个可拖动面板：left 目录树 · right 详情 · file 底部文件面板 */
export type PaneKey = 'left' | 'right' | 'file';

export interface LayoutState {
  size: Record<PaneKey, number>;
  show: Record<PaneKey, boolean>;
  /** 表格列宽（px，键 = th[data-sort]），缺省列走 CSS 默认宽度 */
  colWidths: Record<string, number>;
}

export const PANE_DEFAULT_SIZE: Record<PaneKey, number> = { left: 300, right: 350, file: 200 };

const PANE_LIMIT: Record<PaneKey, [number, number]> = {
  left: [220, 560],
  right: [260, 640],
  file: [96, 640],
};

const STORE_KEY = 'vinepress.layout';
const PANE_KEYS: PaneKey[] = ['left', 'right', 'file'];

function clampPane(key: PaneKey, value: number): number {
  const [lo, hi] = PANE_LIMIT[key];
  if (!Number.isFinite(value)) return PANE_DEFAULT_SIZE[key];
  return Math.min(hi, Math.max(lo, Math.round(value)));
}

function freshLayout(): LayoutState {
  return {
    size: { ...PANE_DEFAULT_SIZE },
    show: { left: true, right: true, file: true },
    colWidths: {},
  };
}

function loadLayout(): LayoutState {
  const base = freshLayout();
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (!raw) return base;
    const parsed = JSON.parse(raw) as Partial<LayoutState>;
    for (const key of PANE_KEYS) {
      const size = parsed.size?.[key];
      const show = parsed.show?.[key];
      if (typeof size === 'number') base.size[key] = clampPane(key, size);
      if (typeof show === 'boolean') base.show[key] = show;
    }
    if (parsed.colWidths && typeof parsed.colWidths === 'object') {
      for (const [k, v] of Object.entries(parsed.colWidths)) {
        if (typeof v === 'number' && Number.isFinite(v) && v >= 40) base.colWidths[k] = Math.round(v);
      }
    }
  } catch {
    // 损坏的偏好 → 用默认值
  }
  return base;
}

let layout: LayoutState = loadLayout();

function persist(): void {
  localStorage.setItem(STORE_KEY, JSON.stringify(layout));
}

/** settings.ts 把缩放倍率写在 :root --zoom；拖动按其折算指针位移 */
function zoomFactor(): number {
  const v = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--zoom'));
  return Number.isFinite(v) && v > 0 ? v : 1;
}

export function paneVisible(key: PaneKey): boolean {
  return layout.show[key];
}

export function paneSize(key: PaneKey): number {
  return layout.size[key];
}

export function applyLayout(): void {
  const ws = el('workspace');
  ws.style.setProperty(
    '--ws-cols',
    [
      layout.show.left ? `${layout.size.left}px` : '0px',
      layout.show.left ? '6px' : '0px',
      'minmax(0, 1fr)',
      layout.show.right ? '6px' : '0px',
      layout.show.right ? `${layout.size.right}px` : '0px',
    ].join(' '),
  );
  ws.style.setProperty('--pane-file', `${layout.show.file ? layout.size.file : 0}px`);
  const app = el('app');
  app.classList.toggle('hide-left', !layout.show.left);
  app.classList.toggle('hide-right', !layout.show.right);
  app.classList.toggle('hide-file', !layout.show.file);
  // 面板尺寸/显隐会改变表格容器宽：px 表宽模式下若不重算会出现右侧留白
  fitTableWidth();
}

// ---------------------------------------------------------------- 表格列宽（Excel 式拖拽）

/** 表宽自适应：各数据列 px 宽之和超出容器时给表格 px 宽（出现横向滚动），否则铺满 100%。
 *  填充列 .col-fill 不计入 sum——它靠 width:auto 吸收剩余空间，计入会让恒等式 sum≈容器 而永远不进 px 模式 */
function fitTableWidth(): void {
  const table = el('entry-table');
  const wrap = el('entry-table-wrap');
  const sum = [...table.querySelectorAll<HTMLElement>('thead th:not(.col-fill)')].reduce((a, th) => a + th.offsetWidth, 0);
  table.style.width = sum > wrap.clientWidth - 1 ? `${sum}px` : '100%';
}

/** 把持久化列宽套用到表头（缺省列清掉内联宽度回退 CSS 默认） */
export function applyColWidths(): void {
  const table = el('entry-table');
  table.querySelectorAll<HTMLElement>('thead th[data-sort]').forEach((th) => {
    const w = layout.colWidths[th.dataset.sort!];
    if (typeof w === 'number') th.style.width = `${w}px`;
    else th.style.removeProperty('width');
  });
  fitTableWidth();
}

function persistColWidths(): void {
  persist();
}

/** 列宽拖拽：拖表头右缘改宽（min 48px），双击右缘复位该列 */
export function initColResize(): void {
  const table = el('entry-table');
  applyColWidths();
  table.querySelectorAll<HTMLElement>('.col-resizer').forEach((resizer) => {
    resizer.addEventListener('click', (e) => e.stopPropagation());
    resizer.addEventListener('dblclick', (e) => {
      e.stopPropagation();
      const th = resizer.closest('th') as HTMLElement | null;
      const key = th?.dataset.sort;
      if (!th || !key) return;
      delete layout.colWidths[key];
      th.style.removeProperty('width');
      fitTableWidth();
      persistColWidths();
    });
    resizer.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      const th = resizer.closest('th') as HTMLElement | null;
      if (!th) return;
      e.preventDefault();
      e.stopPropagation();
      resizer.setPointerCapture(e.pointerId);
      // 首次拖动时把 % / auto 列具体化为 px，之后全部按 px 计
      const ths = [...table.querySelectorAll<HTMLElement>('thead th[data-sort]')];
      ths.forEach((t) => {
        t.style.width = `${t.offsetWidth}px`;
      });
      const startX = e.clientX;
      const startW = th.offsetWidth;
      const body = document.body;
      body.classList.add('drag-col');
      const move = (ev: PointerEvent): void => {
        const w = Math.max(48, startW + (ev.clientX - startX) / zoomFactor());
        th.style.width = `${Math.round(w)}px`;
        fitTableWidth();
      };
      const end = (): void => {
        resizer.removeEventListener('pointermove', move);
        resizer.removeEventListener('pointerup', end);
        resizer.removeEventListener('pointercancel', end);
        body.classList.remove('drag-col');
        ths.forEach((t) => {
          const key = t.dataset.sort;
          if (key) layout.colWidths[key] = t.offsetWidth;
        });
        fitTableWidth();
        persistColWidths();
      };
      resizer.addEventListener('pointermove', move);
      resizer.addEventListener('pointerup', end);
      resizer.addEventListener('pointercancel', end);
    });
  });
  window.addEventListener('resize', fitTableWidth);
}

export function setPaneSize(key: PaneKey, value: number, persistNow = true): void {
  layout.size[key] = clampPane(key, value);
  applyLayout();
  if (persistNow) persist();
}

export function togglePane(key: PaneKey): void {
  layout.show[key] = !layout.show[key];
  applyLayout();
  persist();
}

export function resetLayout(): void {
  layout = freshLayout();
  applyLayout();
  applyColWidths();
  persist();
}

export function getLayout(): LayoutState {
  return JSON.parse(JSON.stringify(layout)) as LayoutState;
}

export function restoreLayout(snapshot: LayoutState): void {
  layout = JSON.parse(JSON.stringify(snapshot)) as LayoutState;
  if (!layout.colWidths) layout.colWidths = {};
  applyLayout();
  applyColWidths();
  persist();
}

function wireSplitter(id: string, key: PaneKey, axis: 'x' | 'y', dir: 1 | -1): void {
  const handle = el(id);
  handle.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || !layout.show[key]) return;
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    const base = layout.size[key];
    const start = axis === 'x' ? e.clientX : e.clientY;
    const body = document.body;
    body.classList.add(axis === 'x' ? 'drag-col' : 'drag-row');
    const move = (ev: PointerEvent): void => {
      const cur = axis === 'x' ? ev.clientX : ev.clientY;
      setPaneSize(key, base + ((cur - start) / zoomFactor()) * dir, false);
    };
    const end = (): void => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', end);
      handle.removeEventListener('pointercancel', end);
      body.classList.remove('drag-col', 'drag-row');
      persist();
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', end);
    handle.addEventListener('pointercancel', end);
  });
  handle.addEventListener('dblclick', () => setPaneSize(key, PANE_DEFAULT_SIZE[key]));
}

/** 三根分隔条：拖动改尺寸（钳位）、双击复位该侧；启动时套用已存布局与列宽 */
export function initSplitters(): void {
  wireSplitter('split-left', 'left', 'x', 1);
  wireSplitter('split-right', 'right', 'x', -1);
  wireSplitter('split-file', 'file', 'y', -1);
  applyLayout();
  initColResize();
}
