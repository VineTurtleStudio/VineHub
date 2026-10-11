import type { CategoryNode, CollectionRow, ItemFilter, ItemRow, Overview, RootEntry } from './api';

export type TreeSel =
  | { kind: 'all' }
  | { kind: 'root'; id: number }
  | { kind: 'type'; value: string }
  | { kind: 'category'; id: number }
  | { kind: 'collection'; id: number };

export interface AppState {
  view: 'browse' | 'settings';
  roots: RootEntry[];
  overview: Overview | null;
  categories: CategoryNode[];
  collections: CollectionRow[];
  sel: TreeSel;
  // 智能集合预览（null = 常规列表）
  collection: { name: string; queryJson: string } | null;
  // 列表
  sort: string;
  desc: boolean;
  search: string;
  total: number;
  chunks: Map<number, ItemRow[]>;
  loadingChunks: Set<number>;
  viewMode: 'list' | 'grid';
  selectedId: number | null;
  // 多选（Ctrl/Shift 点击，批量打标用）
  selectedIds: Set<number>;
  // 范围选中的锚点（最近一次单击/Ctrl 点击的行，Shift 点击/Shift 方向键从锚点扩展）
  anchorId: number | null;
  jobRunning: boolean;
}

export const state: AppState = {
  view: 'browse',
  roots: [],
  overview: null,
  categories: [],
  collections: [],
  sel: { kind: 'all' },
  collection: null,
  sort: 'file_name',
  desc: false,
  search: '',
  total: 0,
  chunks: new Map(),
  loadingChunks: new Set(),
  viewMode: 'list',
  selectedId: null,
  selectedIds: new Set(),
  anchorId: null,
  jobRunning: false,
};

export function resetChunks(): void {
  state.chunks.clear();
  state.loadingChunks.clear();
}

export function resetList(): void {
  resetChunks();
  state.selectedId = null;
  state.selectedIds.clear();
  state.anchorId = null;
}

/// 当前树选择 → 后端过滤器（集合预览由 ensureChunk 走 previewCollection）
export function currentFilter(): ItemFilter {
  const f: ItemFilter = { sort: state.sort, desc: state.desc };
  if (state.sel.kind === 'root') f.root_id = state.sel.id;
  else if (state.sel.kind === 'type') f.type_name = state.sel.value;
  else if (state.sel.kind === 'category') f.category_id = state.sel.id;
  return f;
}

export const CHUNK = 500;
