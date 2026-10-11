import {
  getItem,
  getOverview,
  listCategories,
  listCollections,
  listItems,
  openPath,
  previewCollection,
  revealPath,
  searchItems,
  ensureThumb,
  setItemFields,
  type CategoryNode,
  type ItemDetail,
  type ItemRow,
} from './api';
import { convertFileSrc } from '@tauri-apps/api/core';
import { CHUNK, currentFilter, resetChunks, resetList, state, type TreeSel } from './state';
import { dropCategory } from './features/tree';
import { addTagsToItem, removeTagFromItem } from './features/collections';
import { setItemCategories } from './api';

// ---------------------------------------------------------------- DOM

const treeBody = document.getElementById('tree-body') as HTMLElement;
const listHeader = document.getElementById('list-header') as HTMLElement;
const listScroll = document.getElementById('list-scroll') as HTMLElement;
const listSpacer = document.getElementById('list-spacer') as HTMLElement;
const listRows = document.getElementById('list-rows') as HTMLElement;
const listEmpty = document.getElementById('list-empty') as HTMLElement;
const listCount = document.getElementById('list-count') as HTMLElement;
const detailBody = document.getElementById('detail-body') as HTMLElement;
const viewListBtn = document.getElementById('view-list') as HTMLButtonElement;
const viewGridBtn = document.getElementById('view-grid') as HTMLButtonElement;
const gridSortBar = document.getElementById('grid-sort-bar') as HTMLElement;
const gridSort = document.getElementById('grid-sort') as HTMLSelectElement;
const gridSortDir = document.getElementById('grid-sort-dir') as HTMLButtonElement;

const ROW_H = 28;
const OVERSCAN = 8;
// 网格视图：卡片固定高度（CSS .gcard 同步）+ 列间距 + 最小卡宽
const GRID_CARD_H = 200;
const GRID_GAP = 12;
const GRID_MIN_W = 176;

export const TYPE_LABELS: Record<string, string> = {
  music: '音乐',
  video: '视频',
  book: '图书',
  comic: '漫画',
  image: '图片',
  other: '其他',
};

export const STATUS_LABELS: Record<string, string> = {
  inbox: '收件箱',
  unwatched: '未看',
  reading: '在读',
  done: '完成',
  dropped: '弃坑',
};

// ---------------------------------------------------------------- 树

export function renderTree(): void {
  treeBody.innerHTML = '';

  const mkSection = (title: string): HTMLElement => {
    const h = document.createElement('div');
    h.className = 'tree-section';
    h.textContent = title;
    return h;
  };
  const mkRow = (
    sel: TreeSel,
    label: string,
    count?: number,
    opts?: { depth?: number; dragId?: number },
  ): HTMLElement => {
    const row = document.createElement('div');
    row.className = 'tree-row';
    if (sameSel(state.sel, sel)) row.classList.add('active');
    const name = document.createElement('span');
    name.className = 'tree-name';
    name.textContent = label;
    const badge = document.createElement('span');
    badge.className = 'tree-count';
    badge.textContent = count !== undefined ? String(count) : '';
    row.append(name, badge);
    if (opts?.depth !== undefined) row.style.paddingLeft = `${8 + opts.depth * 14}px`;
    if (opts?.dragId !== undefined) {
      row.draggable = true;
      row.dataset.catId = String(opts.dragId);
      row.addEventListener('dragstart', (e) => {
        e.dataTransfer?.setData('text/vine-cat', String(opts.dragId));
        e.dataTransfer && (e.dataTransfer.effectAllowed = 'move');
      });
      // 拖到分类行 = 挂到该分类下
      row.addEventListener('dragover', (e) => {
        if (e.dataTransfer?.types.includes('text/vine-cat')) {
          e.preventDefault();
          row.classList.add('drop-hint');
        }
      });
      row.addEventListener('dragleave', () => row.classList.remove('drop-hint'));
      row.addEventListener('drop', (e) => {
        e.preventDefault();
        row.classList.remove('drop-hint');
        const src = Number(e.dataTransfer?.getData('text/vine-cat'));
        if (src) void dropCategory(src, opts.dragId ?? null, refreshTreeAndList);
      });
    }
    row.addEventListener('click', () => {
      // 树导航 = 退出搜索模式（否则 ensureChunk 搜索优先，树点击看起来无效）
      if (state.search) {
        state.search = '';
        const si = document.getElementById('search-input') as HTMLInputElement;
        si.value = '';
      }
      state.sel = sel;
      if (sel.kind === 'collection') {
        const c = state.collections.find((x) => x.id === sel.id);
        state.collection = c ? { name: c.name, queryJson: c.query_json } : null;
      } else {
        state.collection = null;
      }
      resetList();
      renderTree();
      void reloadList();
      void renderDetail(null);
    });
    return row;
  };

  // 全部
  treeBody.appendChild(mkRow({ kind: 'all' }, '全部条目', state.overview?.total));

  // 根目录
  if (state.roots.length > 0) {
    treeBody.appendChild(mkSection('根目录'));
    for (const r of state.roots) {
      treeBody.appendChild(
        mkRow({ kind: 'root', id: r.id }, r.label || r.path.replace(/^.*[\\/]/, ''), r.item_count),
      );
    }
  }

  // 类型
  if (state.overview && state.overview.by_type.length > 0) {
    treeBody.appendChild(mkSection('类型'));
    for (const t of state.overview.by_type) {
      treeBody.appendChild(
        mkRow({ kind: 'type', value: t.key }, TYPE_LABELS[t.key] ?? t.key, t.count),
      );
    }
  }

  // 分类树
  treeBody.appendChild(mkSection('分类'));
  const walk = (nodes: CategoryNode[], depth: number): void => {
    for (const n of nodes) {
      treeBody.appendChild(
        mkRow({ kind: 'category', id: n.id }, n.name, n.item_count, {
          depth,
          dragId: n.id,
        }),
      );
      if (n.children.length > 0) walk(n.children, depth + 1);
    }
  };
  walk(state.categories, 0);
  // 拖到分类区空白 = 移到顶层
  const catZone = document.createElement('div');
  catZone.className = 'tree-row tree-dropzone';
  catZone.textContent = '（拖到这里 = 顶层）';
  catZone.addEventListener('dragover', (e) => {
    if (e.dataTransfer?.types.includes('text/vine-cat')) {
      e.preventDefault();
      catZone.classList.add('drop-hint');
    }
  });
  catZone.addEventListener('dragleave', () => catZone.classList.remove('drop-hint'));
  catZone.addEventListener('drop', (e) => {
    e.preventDefault();
    catZone.classList.remove('drop-hint');
    const src = Number(e.dataTransfer?.getData('text/vine-cat'));
    if (src) void dropCategory(src, null, refreshTreeAndList);
  });
  treeBody.appendChild(catZone);

  // 智能集合
  if (state.collections.length > 0) {
    treeBody.appendChild(mkSection('智能集合'));
    for (const c of state.collections) {
      treeBody.appendChild(mkRow({ kind: 'collection', id: c.id }, c.name, undefined, {}));
    }
  }
}

/// 树操作后的统一刷新（分类 CRUD / 拖拽）
export async function refreshTreeAndList(): Promise<void> {
  state.categories = await listCategories();
  state.collections = await listCollections();
  renderTree();
  await reloadList();
}

function sameSel(a: TreeSel, b: TreeSel): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'root' && b.kind === 'root') return a.id === b.id;
  if (a.kind === 'type' && b.kind === 'type') return a.value === b.value;
  if (a.kind === 'category' && b.kind === 'category') return a.id === b.id;
  if (a.kind === 'collection' && b.kind === 'collection') return a.id === b.id;
  return true;
}

// ---------------------------------------------------------------- 列表头

interface Column {
  key: string;
  label: string;
  width: number; // 0 = 自适应
}

export const COLUMNS: Column[] = [
  { key: 'file_name', label: '名称', width: 0 },
  { key: 'type_name', label: '类型', width: 72 },
  { key: 'size', label: '大小', width: 88 },
  { key: 'year', label: '年份', width: 64 },
  { key: 'rating', label: '评分', width: 56 },
  { key: 'status', label: '状态', width: 80 },
  { key: 'mtime', label: '修改时间', width: 132 },
];

export function renderHeader(): void {
  listHeader.innerHTML = '';
  for (const col of COLUMNS) {
    const th = document.createElement('div');
    th.className = 'th';
    th.dataset.col = col.key;
    if (col.key === 'file_name') th.classList.add('grow');
    else th.style.width = `${col.width}px`;
    const active = sortKey() === col.key;
    if (active) th.classList.add('sorted');
    th.textContent = active ? `${col.label} ${state.desc ? '↓' : '↑'}` : col.label;
    th.title = '点击排序';
    th.addEventListener('click', () => applySort(col));
    listHeader.appendChild(th);
  }
}

/// 列头 / 网格排序下拉共用的排序入口（键名与后端白名单一致）
function applySort(col: Column): void {
  if (sortKey() === col.key) state.desc = !state.desc;
  else {
    state.sort = col.key;
    state.desc = col.key === 'file_name' || col.key === 'type_name' ? false : true;
  }
  resetList();
  renderHeader();
  gridSort.value = state.sort;
  gridSortDir.textContent = state.desc ? '↓' : '↑';
  void reloadList();
}

function sortKey(): string {
  // 后端接受的排序键与列 key 一致（type_name 后端别名 type）
  return state.sort;
}

// ---------------------------------------------------------------- 数据加载

function chunkOf(index: number): number {
  return Math.floor(index / CHUNK);
}

/// 返回 true = 拉到了新数据（调用方据此决定是否重渲染；已缓存返回 false——
/// 否则 `.then(renderViewport)` 会形成微任务死循环冻结页面）。
/// 并发等待同一块时共用同一 Promise（预取进行中也能拿到行，不会早退丢选中）。
const chunkLoads = new Map<number, Promise<boolean>>();

async function ensureChunk(ci: number): Promise<boolean> {
  if (state.chunks.has(ci)) return false;
  const existing = chunkLoads.get(ci);
  if (existing) return existing;
  const p = loadChunk(ci);
  chunkLoads.set(ci, p);
  try {
    return await p;
  } finally {
    chunkLoads.delete(ci);
  }
}

async function loadChunk(ci: number): Promise<boolean> {
  state.loadingChunks.add(ci);
  try {
    if (state.search) {
      // 搜索模式（优先级最高）：一次性取前 1000 条，客户端切块 + 本地排序
      if (state.chunks.size > 0) return false;
      const rows = await searchItems(state.search, 1000);
      applyLocalSort(rows);
      state.total = rows.length;
      sliceIntoChunks(rows);
      return true;
    }
    if (state.collection) {
      // 集合预览：一次取全量（后端 limit 钳制 ≤1000），客户端切块 + 本地排序
      if (state.chunks.size > 0) return false;
      const [total, rows] = await previewCollection(state.collection.queryJson, null);
      applyLocalSort(rows);
      state.total = total;
      sliceIntoChunks(rows);
      return true;
    }
    const page = await listItems({
      ...currentFilter(),
      offset: ci * CHUNK,
      limit: CHUNK,
    });
    state.total = page.total;
    state.chunks.set(ci, page.items);
    if (ci === 0 && page.total === 0) state.chunks.set(0, []);
    return true;
  } finally {
    state.loadingChunks.delete(ci);
  }
}

function rowAt(i: number): ItemRow | null {
  const c = state.chunks.get(chunkOf(i));
  if (!c) return null;
  return c[i % CHUNK] ?? null;
}

// 搜索/集合模式在客户端切块，排序也走本地（后端排序只对分页模式生效）
function applyLocalSort(rows: ItemRow[]): void {
  const key = state.sort as keyof ItemRow;
  const mul = state.desc ? -1 : 1;
  rows.sort((a, b) => {
    const av = a[key];
    const bv = b[key];
    if (typeof av === 'number' && typeof bv === 'number') return (av - bv) * mul;
    if (av === null || av === undefined) return bv === null || bv === undefined ? 0 : mul;
    if (bv === null || bv === undefined) return -mul;
    return String(av).localeCompare(String(bv)) * mul;
  });
}

function sliceIntoChunks(rows: ItemRow[]): void {
  state.chunks.clear();
  for (let i = 0; i < rows.length; i += CHUNK) {
    state.chunks.set(Math.floor(i / CHUNK), rows.slice(i, i + CHUNK));
  }
  if (state.chunks.size === 0) state.chunks.set(0, []);
}

export async function reloadList(): Promise<void> {
  updateCount();
  await ensureChunk(0);
  renderViewport();
  updateCount();
}

function updateCount(): void {
  let label = '';
  if (state.collection) label = `集合 “${state.collection.name}” · `;
  else if (state.search) label = `搜索 “${state.search}” · `;
  listCount.textContent = `${label}${state.total} 条`;
  const empty = state.total === 0;
  listEmpty.hidden = !empty;
  if (!empty) return;
  listEmpty.innerHTML = '';
  if (state.roots.length === 0 && !state.search && !state.collection) {
    // 首次使用：还没有媒体根目录 → 引导 CTA
    listEmpty.classList.add('with-cta');
    const p = document.createElement('p');
    p.textContent = '还没有媒体根目录——添加一个外部目录，开始建立只读索引。';
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'btn';
    btn.textContent = '添加根目录…';
    btn.addEventListener('click', () => {
      window.dispatchEvent(new CustomEvent('vc:goto-roots'));
    });
    listEmpty.append(p, btn);
  } else {
    listEmpty.classList.remove('with-cta');
    listEmpty.textContent = state.search ? '搜索无结果' : '无匹配条目';
  }
}

// ---------------------------------------------------------------- 虚拟滚动渲染

function renderViewport(): void {
  if (state.viewMode === 'grid') {
    renderGrid();
    return;
  }
  // 列表模式：清掉网格模式可能留下的内联布局
  listRows.style.display = '';
  listRows.style.gridTemplateColumns = '';
  listRows.style.gap = '';
  const total = state.total;
  listSpacer.style.height = `${total * ROW_H}px`;
  const scrollTop = listScroll.scrollTop;
  const viewH = listScroll.clientHeight || 600;
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(total, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN);

  // 预取相邻块（仅新拉到数据时重渲染，防微任务死循环）
  const firstCi = chunkOf(start);
  const lastCi = chunkOf(Math.max(start, end - 1));
  for (let ci = firstCi; ci <= lastCi; ci++) {
    void ensureChunk(ci).then((fresh) => {
      if (fresh) renderViewport();
    });
  }
  // 预取下一块（滚动方向外推）
  void ensureChunk(lastCi + 1);

  listRows.style.transform = `translateY(${start * ROW_H}px)`;
  listRows.innerHTML = '';
  for (let i = start; i < end; i++) {
    const row = rowAt(i);
    const el = document.createElement('div');
    el.className = 'lrow';
    if (row) {
      if (row.id === state.selectedId || state.selectedIds.has(row.id)) {
        el.classList.add('selected');
      }
      if (row.missing) el.classList.add('missing');
      el.dataset.id = String(row.id);
      el.style.height = `${ROW_H}px`;
      el.appendChild(cell(row.file_name, true, state.search));
      el.appendChild(cell(TYPE_LABELS[row.type_name] ?? row.type_name));
      el.appendChild(cell(formatSize(row.size)));
      el.appendChild(cell(row.year ? String(row.year) : ''));
      el.appendChild(cell(row.rating > 0 ? '★'.repeat(row.rating) : ''));
      el.appendChild(cell(STATUS_LABELS[row.status] ?? row.status));
      el.appendChild(cell(formatTime(row.mtime)));
      if (row.missing) el.title = '文件缺失';
      wireRow(el, row);
      el.appendChild(buildRowActions(row));
    } else {
      el.style.height = `${ROW_H}px`;
      el.appendChild(cell('…'));
    }
    listRows.appendChild(el);
  }
}

// ---------------------------------------------------------------- 网格视图（封面墙）

function gridCols(): number {
  const w = listScroll.clientWidth || 800;
  return Math.max(2, Math.floor((w + GRID_GAP) / (GRID_MIN_W + GRID_GAP)));
}

function gridUnitH(): number {
  return GRID_CARD_H + GRID_GAP;
}

function renderGrid(): void {
  const total = state.total;
  const cols = gridCols();
  const unit = gridUnitH();
  const rows = Math.ceil(total / cols);
  listSpacer.style.height = `${rows * unit}px`;
  const scrollTop = listScroll.scrollTop;
  const viewH = listScroll.clientHeight || 600;
  const startRow = Math.max(0, Math.floor(scrollTop / unit) - OVERSCAN);
  const endRow = Math.min(rows, Math.ceil((scrollTop + viewH) / unit) + OVERSCAN);
  const start = startRow * cols;
  const end = Math.min(total, endRow * cols);

  // 预取相邻块 + 下一块（仅新拉到数据时重渲染）
  const firstCi = chunkOf(Math.max(0, start));
  const lastCi = chunkOf(Math.max(0, end - 1));
  for (let ci = firstCi; ci <= lastCi; ci++) {
    void ensureChunk(ci).then((fresh) => {
      if (fresh) renderViewport();
    });
  }
  void ensureChunk(lastCi + 1);

  listRows.style.transform = `translateY(${startRow * unit}px)`;
  listRows.style.display = 'grid';
  listRows.style.gridTemplateColumns = `repeat(${cols}, minmax(0, 1fr))`;
  listRows.style.gap = `${GRID_GAP}px`;
  listRows.innerHTML = '';
  for (let i = start; i < end; i++) {
    const row = rowAt(i);
    if (row) listRows.appendChild(gridCard(row));
    else {
      const ph = document.createElement('div');
      ph.className = 'gcard loading';
      ph.textContent = '…';
      listRows.appendChild(ph);
    }
  }
}

function gridCard(row: ItemRow): HTMLElement {
  const el = document.createElement('div');
  el.className = 'gcard';
  if (row.id === state.selectedId || state.selectedIds.has(row.id)) el.classList.add('selected');
  if (row.missing) el.classList.add('missing');
  el.dataset.id = String(row.id);
  if (row.missing) el.title = '文件缺失';

  const thumb = document.createElement('div');
  thumb.className = 'gcard-thumb';
  const ph = document.createElement('div');
  ph.className = 'gthumb-ph';
  ph.textContent = TYPE_LABELS[row.type_name] ?? row.type_name;
  thumb.appendChild(ph);
  thumb.appendChild(buildRowActions(row));

  const name = document.createElement('div');
  name.className = 'gcard-name';
  name.title = row.file_name;
  if (state.search) appendHighlight(name, row.file_name, state.search);
  else name.textContent = row.file_name;

  const meta = document.createElement('div');
  meta.className = 'gcard-meta';
  const bits: string[] = [TYPE_LABELS[row.type_name] ?? row.type_name];
  if (row.year) bits.push(String(row.year));
  if (row.rating > 0) bits.push('★'.repeat(row.rating));
  const size = formatSize(row.size);
  if (size) bits.push(size);
  meta.textContent = bits.join(' · ');
  meta.title = meta.textContent;

  el.append(thumb, name, meta);
  wireRow(el, row);
  loadThumb(thumb, row);
  return el;
}

// 封面懒加载：`<id>:<mtime>` 键缓存（mtime 变化自动重取），进行中去重
const thumbCache = new Map<string, string>();
const thumbInflight = new Map<string, Promise<string>>();

function loadThumb(holder: HTMLElement, row: ItemRow): void {
  if (row.type_name !== 'image' && row.type_name !== 'video') return;
  const key = `${row.id}:${row.mtime}`;
  const cached = thumbCache.get(key);
  if (cached !== undefined) {
    applyThumb(holder, cached);
    return;
  }
  let p = thumbInflight.get(key);
  if (!p) {
    p = ensureThumb(row.id)
      .then((r) => {
        const url = convertFileSrc(r.path);
        thumbCache.set(key, url);
        return url;
      })
      .catch((err) => {
        thumbCache.set(key, ''); // 失败也不重试（同一 mtime）
        throw err;
      })
      .finally(() => thumbInflight.delete(key));
    thumbInflight.set(key, p);
  }
  p.then((url) => {
    if (holder.isConnected) applyThumb(holder, url);
  }).catch(() => undefined);
}

function applyThumb(holder: HTMLElement, url: string): void {
  if (!url) return;
  let img = holder.querySelector('img');
  if (!img) {
    img = document.createElement('img');
    img.alt = '';
    holder.appendChild(img);
  }
  img.src = url;
}

function cell(text: string, grow = false, highlightQuery?: string): HTMLElement {
  const td = document.createElement('div');
  td.className = grow ? 'td grow' : 'td';
  td.title = text;
  if (highlightQuery) appendHighlight(td, text, highlightQuery);
  else td.textContent = text;
  return td;
}

/// 把 text 中命中 query 词干的部分包进 <mark>（大小写不敏感，纯文本节点防注入）
function appendHighlight(parent: HTMLElement, text: string, query: string): void {
  const terms = query
    .trim()
    .split(/\s+/)
    .filter((t) => t.length > 0)
    .map((t) => t.toLowerCase());
  if (terms.length === 0) {
    parent.textContent = text;
    return;
  }
  const lower = text.toLowerCase();
  let pos = 0;
  while (pos < text.length) {
    let hit = -1;
    let hitLen = 0;
    for (const term of terms) {
      const idx = lower.indexOf(term, pos);
      if (idx !== -1 && (hit === -1 || idx < hit)) {
        hit = idx;
        hitLen = term.length;
      }
    }
    if (hit === -1) {
      parent.appendChild(document.createTextNode(text.slice(pos)));
      return;
    }
    if (hit > pos) {
      parent.appendChild(document.createTextNode(text.slice(pos, hit)));
    }
    const mark = document.createElement('mark');
    mark.textContent = text.slice(hit, hit + hitLen);
    parent.appendChild(mark);
    pos = hit + hitLen;
  }
}

export function formatSize(bytes: number): string {
  if (bytes <= 0) return '';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 || u === 0 ? Math.round(v) : v.toFixed(1)} ${units[u]}`;
}

export function formatTime(ms: number): string {
  if (ms <= 0) return '';
  const d = new Date(ms);
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// ---------------------------------------------------------------- 行操作（打开 / 右键菜单 / 键盘导航）

const ICON_OPEN =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2.5H13.5V6.5"/><path d="M13.5 2.5 8 8"/><path d="M12.5 9.5V13a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1H6.5"/></svg>';
const ICON_LOCATE =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M1.8 7.2V13a1 1 0 0 0 1 1h4.4l1.4 1.6h3.6l1.4-1.6h1a1 1 0 0 0 1-1V7.2Z"/><path d="M6.2 1.8H2.8a1 1 0 0 0-1 1v4.4l1.6 1.4 1.4-1.6"/></svg>';

/// root 形如 `F:\` 时 `${root}/${rel}` 会产生 `F:\/...`——统一以 / 拼接并防重复分隔
function joinRoot(rootPath: string, relPath: string): string {
  const rel = relPath.replace(/\\/g, '/');
  if (!rootPath) return rel;
  const sep = /[\\/]$/.test(rootPath) ? '' : '/';
  return `${rootPath}${sep}${rel}`.replace(/\\/g, '/');
}

function rowFullPath(row: ItemRow): string {
  const rootPath = state.roots.find((r) => r.id === row.root_id)?.path ?? '';
  return joinRoot(rootPath, row.rel_path);
}

// ---------------------------------------------------------------- 预览灯箱（原片只读，资产协议直连根目录）

const lightboxEl = document.getElementById('lightbox') as HTMLElement;
const lightboxStage = document.getElementById('lightbox-stage') as HTMLElement;
const lightboxCaption = document.getElementById('lightbox-caption') as HTMLElement;
const lightboxClose = document.getElementById('lightbox-close') as HTMLElement;

function closeLightbox(): void {
  if (lightboxEl.hidden) return;
  lightboxEl.hidden = true;
  lightboxStage.innerHTML = '';
}

function openLightbox(
  fullPath: string,
  type: 'image' | 'video',
  caption: string,
  poster = '',
): void {
  lightboxStage.innerHTML = '';
  lightboxCaption.textContent = caption;
  if (type === 'video') {
    const v = document.createElement('video');
    v.controls = true;
    v.src = convertFileSrc(fullPath);
    if (poster) v.poster = poster;
    v.addEventListener('error', () => {
      // WebView2 解不了的封装/编码（mkv、hevc 等）→ 封面兜底
      if (poster) {
        lightboxStage.innerHTML = '';
        const img = document.createElement('img');
        img.src = poster;
        img.alt = caption;
        lightboxStage.appendChild(img);
        lightboxCaption.textContent = `${caption}（无法直接播放，显示封面）`;
      } else {
        lightboxCaption.textContent = `${caption}（无法预览：格式不受支持）`;
      }
    });
    lightboxStage.appendChild(v);
  } else {
    const img = document.createElement('img');
    img.src = convertFileSrc(fullPath);
    img.alt = caption;
    img.addEventListener('error', () => {
      lightboxCaption.textContent = `${caption}（预览加载失败：文件缺失或格式不受支持）`;
    });
    lightboxStage.appendChild(img);
  }
  lightboxEl.hidden = false;
}

async function previewRow(row: ItemRow): Promise<void> {
  const full = rowFullPath(row);
  const caption = `${row.file_name} — ${full}`;
  if (row.type_name === 'video') {
    let poster = '';
    try {
      const r = await ensureThumb(row.id);
      poster = convertFileSrc(r.path);
    } catch {
      // 无封面/缺 ffmpeg 照常试播
    }
    openLightbox(full, 'video', caption, poster);
  } else {
    openLightbox(full, 'image', caption);
  }
}

/// 单选/范围/Ctrl 多选 + 双击打开 + 右键菜单（列表行与网格卡片共用）
function wireRow(el: HTMLElement, row: ItemRow): void {
  el.addEventListener('click', (ev) => {
    if (ev.shiftKey && state.anchorId !== null) {
      // Shift 范围连选：锚点 → 当前行（现代列表标准行为）
      const a = indexOfId(state.anchorId);
      const b = indexOfId(row.id);
      if (a !== -1 && b !== -1) {
        applyRange(a, b, row);
        return;
      }
    }
    const multi = ev.ctrlKey || ev.metaKey;
    if (multi) {
      // Ctrl 点击 = 单条切换（批量打标 / 批量设状态），锚点跟随
      if (state.selectedIds.has(row.id)) state.selectedIds.delete(row.id);
      else state.selectedIds.add(row.id);
      state.selectedId = state.selectedIds.has(row.id) ? row.id : null;
      state.anchorId = row.id;
    } else {
      state.selectedIds.clear();
      state.selectedIds.add(row.id);
      state.selectedId = row.id;
      state.anchorId = row.id;
    }
    renderViewport();
    void renderDetail(multi ? null : row.id);
    window.dispatchEvent(new CustomEvent('vc:selection-changed'));
  });
  el.addEventListener('dblclick', () => openRow(row));
  el.addEventListener('contextmenu', (ev) => {
    ev.preventDefault();
    showRowContext(row, ev.clientX, ev.clientY);
  });
}

/// hover 快捷操作（打开 / 定位），列表行放右侧、网格卡片放封面右上角
function buildRowActions(row: ItemRow): HTMLElement {
  const actions = document.createElement('div');
  actions.className = 'row-actions';
  const mkAct = (title: string, icon: string, fn: () => void): void => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'row-act';
    b.title = title;
    b.innerHTML = icon;
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      fn();
    });
    actions.appendChild(b);
  };
  mkAct('打开', ICON_OPEN, () => openRow(row));
  mkAct('定位', ICON_LOCATE, () => {
    revealPath(rowFullPath(row)).catch((err) => toastDetailError(String(err)));
  });
  return actions;
}

function openRow(row: ItemRow): void {
  openPath(rowFullPath(row)).catch((err) => toastDetailError(String(err)));
}

let ctxMenu: HTMLElement | null = null;

function closeCtxMenu(): void {
  if (ctxMenu) {
    ctxMenu.remove();
    ctxMenu = null;
  }
}

function showRowContext(row: ItemRow, x: number, y: number): void {
  closeCtxMenu();
  // 右键到未选中的行 = 先单选它
  if (state.selectedId !== row.id && !state.selectedIds.has(row.id)) {
    state.selectedIds.clear();
    state.selectedIds.add(row.id);
    state.selectedId = row.id;
    renderViewport();
    void renderDetail(row.id);
    window.dispatchEvent(new CustomEvent('vc:selection-changed'));
  }
  const menu = document.createElement('div');
  menu.className = 'ctx-menu';
  const item = (label: string, fn: () => void): void => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'ctx-item';
    b.textContent = label;
    b.addEventListener('click', () => {
      closeCtxMenu();
      fn();
    });
    menu.appendChild(b);
  };
  item('打开', () => openRow(row));
  item('在资源管理器中定位', () => {
    revealPath(rowFullPath(row)).catch((err) => toastDetailError(String(err)));
  });
  if (row.type_name === 'image' || row.type_name === 'video') {
    item('预览', () => {
      void previewRow(row);
    });
  }
  item('复制路径', () => {
    navigator.clipboard
      .writeText(rowFullPath(row))
      .then(() => window.dispatchEvent(new CustomEvent('vc:toast', { detail: ['已复制路径'] })))
      .catch((err) => toastDetailError(String(err)));
  });
  document.body.appendChild(menu);
  const mw = menu.offsetWidth;
  const mh = menu.offsetHeight;
  menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - mw - 6))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - mh - 6))}px`;
  ctxMenu = menu;
}

function indexOfId(id: number | null): number {
  if (id === null) return -1;
  for (const [ci, rows] of state.chunks) {
    const idx = rows.findIndex((r) => r.id === id);
    if (idx !== -1) return ci * CHUNK + idx;
  }
  return -1;
}

function ensureIndexVisible(i: number): void {
  const g = state.viewMode === 'grid';
  const cols = g ? gridCols() : 1;
  const unit = g ? gridUnitH() : ROW_H;
  const top = Math.floor(i / cols) * unit;
  const vh = listScroll.clientHeight;
  if (top < listScroll.scrollTop) listScroll.scrollTop = top;
  else if (top + unit > listScroll.scrollTop + vh) listScroll.scrollTop = top + unit - vh;
}

/// 锚点 a 到 b 的连续范围选中（含端点），焦点落在 focus 行
function applyRange(a: number, b: number, focus: ItemRow): void {
  state.selectedIds.clear();
  const s = Math.min(a, b);
  const e = Math.max(a, b);
  for (let i = s; i <= e; i++) {
    const r = rowAt(i);
    if (r) state.selectedIds.add(r.id);
  }
  state.selectedId = focus.id;
  renderViewport();
  void renderDetail(focus.id);
  window.dispatchEvent(new CustomEvent('vc:selection-changed'));
}

async function selectIndex(next: number, mode: 'single' | 'toggle' | 'range'): Promise<void> {
  if (state.total === 0) return;
  const i = Math.max(0, Math.min(state.total - 1, next));
  if (i === indexOfId(state.selectedId) && mode === 'single') return;
  await ensureChunk(chunkOf(i));
  const row = rowAt(i);
  if (!row) return;
  if (mode === 'range') {
    const anchorIdx =
      state.anchorId !== null ? indexOfId(state.anchorId) : indexOfId(state.selectedId);
    applyRange(anchorIdx === -1 ? i : anchorIdx, i, row);
    ensureIndexVisible(i);
    return;
  }
  if (mode === 'toggle') {
    if (state.selectedIds.has(row.id)) state.selectedIds.delete(row.id);
    else state.selectedIds.add(row.id);
    state.selectedId = state.selectedIds.has(row.id) ? row.id : null;
    state.anchorId = row.id;
  } else {
    state.selectedIds.clear();
    state.selectedIds.add(row.id);
    state.selectedId = row.id;
    state.anchorId = row.id;
  }
  renderViewport();
  ensureIndexVisible(i);
  void renderDetail(state.selectedId);
  window.dispatchEvent(new CustomEvent('vc:selection-changed'));
}

/// 列表键盘导航：↑↓/PgUp/PgDn/Home/End 移动选中（Shift 范围连选、Ctrl 单条切换），Enter 打开
/// 网格模式：↑↓ 按行跨列（±cols）、←→ 逐卡、Page 按可视行数
async function onListKey(e: KeyboardEvent): Promise<void> {
  const tag = (e.target as HTMLElement).tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
  const mode = e.shiftKey ? 'range' : e.ctrlKey || e.metaKey ? 'toggle' : 'single';
  const g = state.viewMode === 'grid';
  const cols = g ? gridCols() : 1;
  const unit = g ? gridUnitH() : ROW_H;
  const pageRows = Math.max(1, Math.floor(listScroll.clientHeight / unit) - 1);
  const page = pageRows * cols;
  const step = cols;
  const cur = indexOfId(state.selectedId);
  const base = cur === -1 ? 0 : cur;
  switch (e.key) {
    case 'ArrowDown':
      e.preventDefault();
      await selectIndex(cur === -1 ? 0 : cur + step, mode);
      break;
    case 'ArrowUp':
      e.preventDefault();
      await selectIndex(cur <= 0 ? 0 : cur - step, mode);
      break;
    case 'ArrowLeft':
      e.preventDefault();
      await selectIndex(cur <= 0 ? 0 : cur - 1, mode);
      break;
    case 'ArrowRight':
      e.preventDefault();
      await selectIndex(cur === -1 ? 0 : cur + 1, mode);
      break;
    case 'PageDown':
      e.preventDefault();
      await selectIndex(base + page, mode);
      break;
    case 'PageUp':
      e.preventDefault();
      await selectIndex(base - page, mode);
      break;
    case 'Home':
      e.preventDefault();
      await selectIndex(0, mode);
      break;
    case 'End':
      e.preventDefault();
      await selectIndex(state.total - 1, mode);
      break;
    case 'Enter': {
      e.preventDefault();
      if (cur < 0) return;
      await ensureChunk(chunkOf(cur));
      const row = rowAt(cur);
      if (row) openRow(row);
      break;
    }
  }
}

// ---------------------------------------------------------------- 详情写入（状态/评分）

async function patchFields(
  id: number,
  fields: { status?: string | null; rating?: number | null },
): Promise<void> {
  try {
    await setItemFields([id], fields);
    // 本地修补列表行，避免整表重拉
    for (const rows of state.chunks.values()) {
      const r = rows.find((x) => x.id === id);
      if (!r) continue;
      if (fields.status !== undefined && fields.status !== null) r.status = fields.status;
      if (fields.rating !== undefined && fields.rating !== null) r.rating = fields.rating;
    }
    renderViewport();
    if (fields.status !== undefined) {
      // 状态影响树区 by_status 计数
      state.overview = await getOverview(null);
      renderTree();
    }
    await renderDetail(id);
  } catch (err) {
    toastDetailError(String(err));
  }
}

export async function renderDetail(id: number | null): Promise<void> {
  if (id === null) {
    detailBody.innerHTML = '<p class="hint">选择条目查看详情</p>';
    return;
  }
  let d: ItemDetail;
  try {
    d = await getItem(id);
  } catch (err) {
    detailBody.innerHTML = '';
    const p = document.createElement('p');
    p.className = 'hint err';
    p.textContent = String(err);
    detailBody.appendChild(p);
    return;
  }
  detailBody.innerHTML = '';

  const title = document.createElement('h3');
  title.className = 'detail-title';
  if (state.search) appendHighlight(title, d.item.file_name, state.search);
  else title.textContent = d.item.file_name;
  detailBody.appendChild(title);
  detailBody.dataset.itemId = String(d.item.id);

  const path = document.createElement('p');
  path.className = 'detail-path';
  path.textContent = `${d.root_path}/${d.item.rel_path}`;
  path.title = path.textContent;
  detailBody.appendChild(path);

  // 缩略图：图片内部缩放 / 视频 ffmpeg 抽帧（首见触发，之后命中派生目录）
  if (d.item.type_name === 'image' || d.item.type_name === 'video') {
    const holder = document.createElement('div');
    holder.className = 'thumb-holder';
    const ph = document.createElement('div');
    ph.className = 'thumb-ph';
    ph.textContent = '生成缩略图…';
    holder.appendChild(ph);
    detailBody.appendChild(holder);
    ensureThumb(d.item.id)
      .then((r) => {
        if (detailBody.dataset.itemId !== String(d.item.id)) return; // 已切换条目
        holder.innerHTML = '';
        const img = document.createElement('img');
        img.className = 'thumb-img';
        img.alt = '';
        img.src = convertFileSrc(r.path);
        img.style.cursor = 'zoom-in';
        img.title = '点击预览原片';
        img.addEventListener('click', () => {
          const full = `${d.root_path}/${d.item.rel_path}`;
          const caption = `${d.item.file_name} — ${full}`;
          if (d.item.type_name === 'video') openLightbox(full, 'video', caption, img.src);
          else openLightbox(full, 'image', caption);
        });
        holder.appendChild(img);
      })
      .catch((err) => {
        ph.textContent = String(err);
      });
  }

  const kv = (k: string, v: string): void => {
    if (!v) return;
    const row = document.createElement('div');
    row.className = 'kv-row';
    const key = document.createElement('span');
    key.className = 'kv-key';
    key.textContent = k;
    const val = document.createElement('span');
    val.className = 'kv-val';
    val.textContent = v;
    row.append(key, val);
    detailBody.appendChild(row);
  };

  kv('类型', TYPE_LABELS[d.item.type_name] ?? d.item.type_name);
  kv('大小', formatSize(d.item.size));
  kv('年份', d.item.year ? String(d.item.year) : '');

  // 评分：点 1-5 星写入，再点当前星清零
  const rtRow = document.createElement('div');
  rtRow.className = 'kv-row';
  const rtKey = document.createElement('span');
  rtKey.className = 'kv-key';
  rtKey.textContent = '评分';
  const rtVal = document.createElement('span');
  rtVal.className = 'kv-val rating-edit';
  for (let i = 1; i <= 5; i++) {
    const star = document.createElement('button');
    star.type = 'button';
    star.className = `star${d.item.rating >= i ? ' on' : ''}`;
    star.textContent = '★';
    star.addEventListener('click', () => {
      void patchFields(d.item.id, { rating: d.item.rating === i ? 0 : i });
    });
    rtVal.appendChild(star);
  }
  rtRow.append(rtKey, rtVal);
  detailBody.appendChild(rtRow);

  // 状态：下拉即写
  const stRow = document.createElement('div');
  stRow.className = 'kv-row';
  const stKey = document.createElement('span');
  stKey.className = 'kv-key';
  stKey.textContent = '状态';
  const stSel = document.createElement('select');
  stSel.className = 'status-edit';
  for (const [k, label] of Object.entries(STATUS_LABELS)) {
    const opt = document.createElement('option');
    opt.value = k;
    opt.textContent = label;
    stSel.appendChild(opt);
  }
  stSel.value = d.item.status;
  stSel.addEventListener('change', () => {
    void patchFields(d.item.id, { status: stSel.value });
  });
  stRow.append(stKey, stSel);
  detailBody.appendChild(stRow);

  kv('修改', formatTime(d.item.mtime));
  kv('哈希', d.item.hash_state === 'done' ? '已计算' : '未计算');
  kv(
    '元数据',
    d.item.meta_state === 'done' ? '已获取' : d.item.meta_state === 'failed' ? '失败' : '未获取',
  );
  kv('缺失', d.item.missing ? '是' : '');

  if (d.ext.length > 0) {
    const h = document.createElement('div');
    h.className = 'detail-sub';
    h.textContent = '扩展信息';
    detailBody.appendChild(h);
    for (const f of d.ext) {
      kv(f.name, f.value ?? '');
    }
  }

  if (d.tags.length > 0) {
    const h = document.createElement('div');
    h.className = 'detail-sub';
    h.textContent = '标签';
    detailBody.appendChild(h);
    const wrap = document.createElement('div');
    wrap.className = 'tag-wrap';
    for (const t of d.tags) {
      const tag = document.createElement('span');
      tag.className = 'tag';
      tag.textContent = t.name;
      // 点击 × 移除标签（只动关联，不删词表）
      const x = document.createElement('button');
      x.type = 'button';
      x.className = 'tag-x';
      x.textContent = '×';
      x.title = '移除标签';
      x.addEventListener('click', () => {
        void removeTagFromItem(d.item.id, t.id)
          .then(() => renderDetail(d.item.id))
          .catch((err) => toastDetailError(String(err)));
      });
      tag.appendChild(x);
      wrap.appendChild(tag);
    }
    detailBody.appendChild(wrap);
  }

  // 追加标签
  const tagAdd = document.createElement('div');
  tagAdd.className = 'tag-add';
  const tagInput = document.createElement('input');
  tagInput.type = 'text';
  tagInput.placeholder = '添加标签，逗号分隔…';
  tagInput.className = 'tag-input';
  const tagBtn = document.createElement('button');
  tagBtn.type = 'button';
  tagBtn.className = 'btn sm';
  tagBtn.textContent = '添加';
  tagBtn.addEventListener('click', () => {
    void addTagsToItem(d.item.id, tagInput.value)
      .then((changed) => {
        if (changed) {
          tagInput.value = '';
          void renderDetail(d.item.id);
        }
      })
      .catch((err) => toastDetailError(String(err)));
  });
  tagInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') tagBtn.click();
  });
  tagAdd.append(tagInput, tagBtn);
  detailBody.appendChild(tagAdd);

  // 分类归属
  const catSub = document.createElement('div');
  catSub.className = 'detail-sub';
  catSub.textContent = '分类';
  detailBody.appendChild(catSub);
  const catWrap = document.createElement('div');
  catWrap.className = 'tag-wrap';
  void assignCategoryUi(d, catWrap);
  detailBody.appendChild(catWrap);

  const actions = document.createElement('div');
  actions.className = 'detail-actions';
  const btnOpen = document.createElement('button');
  btnOpen.type = 'button';
  btnOpen.className = 'btn sm';
  btnOpen.textContent = '打开';
  btnOpen.addEventListener('click', () => {
    void openPath(fullPath(d));
  });
  const btnReveal = document.createElement('button');
  btnReveal.type = 'button';
  btnReveal.className = 'btn-ghost sm';
  btnReveal.textContent = '定位';
  btnReveal.addEventListener('click', () => {
    void revealPath(fullPath(d));
  });
  const btnCopy = document.createElement('button');
  btnCopy.type = 'button';
  btnCopy.className = 'btn-ghost sm';
  btnCopy.textContent = '复制路径';
  btnCopy.addEventListener('click', () => {
    navigator.clipboard
      .writeText(fullPath(d))
      .then(() => window.dispatchEvent(new CustomEvent('vc:toast', { detail: ['已复制路径'] })))
      .catch((err) => toastDetailError(String(err)));
  });
  actions.append(btnOpen, btnReveal, btnCopy);
  detailBody.appendChild(actions);
}

function fullPath(d: ItemDetail): string {
  return joinRoot(d.root_path, d.item.rel_path);
}

function toastDetailError(msg: string): void {
  window.dispatchEvent(new CustomEvent('vc:toast', { detail: [msg, false] }));
}

/// 分类勾选：点击切换归属（整体替换集合）
async function assignCategoryUi(d: ItemDetail, wrap: HTMLElement): Promise<void> {
  const current = new Set(d.category_ids);
  const rows: { id: number; label: string }[] = [];
  const walk = (nodes: CategoryNode[], depth: number): void => {
    for (const n of nodes) {
      rows.push({ id: n.id, label: `${'· '.repeat(depth)}${n.name}` });
      walk(n.children, depth + 1);
    }
  };
  walk(state.categories, 0);
  if (rows.length === 0) {
    const p = document.createElement('span');
    p.className = 'hint';
    p.textContent = '尚无分类（左上角 ＋分类）';
    wrap.appendChild(p);
    return;
  }
  for (const r of rows) {
    const chip = document.createElement('button');
    chip.type = 'button';
    chip.className = `cat-chip${current.has(r.id) ? ' on' : ''}`;
    chip.textContent = r.label;
    chip.addEventListener('click', () => {
      if (current.has(r.id)) current.delete(r.id);
      else current.add(r.id);
      void setItemCategories(d.item.id, [...current])
        .then(() => renderDetail(d.item.id))
        .catch((err) => toastDetailError(String(err)));
    });
    wrap.appendChild(chip);
  }
}

// ---------------------------------------------------------------- 搜索

let searchTimer: number | undefined;

export function onSearchInput(q: string): void {
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    state.search = q.trim();
    resetList();
    void reloadList().then(() => renderViewport());
  }, 200);
}

export function focusSearch(): void {
  const input = document.getElementById('search-input') as HTMLInputElement;
  input.focus();
  input.select();
}

export function initListScroll(): void {
  listScroll.addEventListener(
    'scroll',
    () => {
      renderViewport();
      closeCtxMenu();
    },
    { passive: true },
  );
  window.addEventListener('resize', () => renderViewport());
  listScroll.addEventListener('keydown', (e) => {
    void onListKey(e);
  });
  document.addEventListener('mousedown', (e) => {
    if (ctxMenu && !ctxMenu.contains(e.target as Node)) closeCtxMenu();
  });
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (!lightboxEl.hidden) {
      closeLightbox();
      return;
    }
    closeCtxMenu();
  });
  lightboxEl.addEventListener('click', (e) => {
    if (e.target === lightboxEl) closeLightbox();
  });
  lightboxClose.addEventListener('click', closeLightbox);
}

// ---------------------------------------------------------------- 视图切换（列表 ⇄ 网格）

function syncViewModeUI(): void {
  const g = state.viewMode === 'grid';
  listHeader.hidden = g;
  gridSortBar.hidden = !g;
  viewListBtn.classList.toggle('active', !g);
  viewGridBtn.classList.toggle('active', g);
}

export function setViewMode(v: 'list' | 'grid'): void {
  if (state.viewMode === v) return;
  state.viewMode = v;
  syncViewModeUI();
  listScroll.scrollTop = 0; // 两种视图滚动语义不同 → 回顶
  renderViewport();
  window.dispatchEvent(new CustomEvent('vc:view-mode', { detail: v }));
}

export function initViewToggle(): void {
  gridSort.innerHTML = '';
  for (const c of COLUMNS) {
    const opt = document.createElement('option');
    opt.value = c.key;
    opt.textContent = c.label;
    gridSort.appendChild(opt);
  }
  gridSort.value = state.sort;
  gridSortDir.textContent = state.desc ? '↓' : '↑';
  gridSort.addEventListener('change', () => {
    const col = COLUMNS.find((c) => c.key === gridSort.value);
    if (col) applySort(col);
  });
  gridSortDir.addEventListener('click', () => {
    state.desc = !state.desc;
    gridSortDir.textContent = state.desc ? '↓' : '↑';
    resetList();
    renderHeader();
    void reloadList();
  });
  viewListBtn.addEventListener('click', () => setViewMode('list'));
  viewGridBtn.addEventListener('click', () => setViewMode('grid'));
  syncViewModeUI();
}
