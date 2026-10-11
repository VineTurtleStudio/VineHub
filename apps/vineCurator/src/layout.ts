/// 面板宽度 / 列宽 / 视图偏好持久化（localStorage 键前缀 vinecurator.layout）

const KEY = 'vinecurator.layout';

export interface LayoutPrefs {
  treeWidth: number;
  detailWidth: number;
  detailOpen: boolean;
  columns: Record<string, number>;
  viewMode: 'list' | 'grid';
}

const DEFAULTS: LayoutPrefs = {
  treeWidth: 220,
  detailWidth: 300,
  detailOpen: true,
  columns: {},
  viewMode: 'list',
};

export function loadLayout(): LayoutPrefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { ...DEFAULTS, columns: {} };
    const parsed = JSON.parse(raw) as Partial<LayoutPrefs>;
    return {
      treeWidth: Number(parsed.treeWidth) > 0 ? Number(parsed.treeWidth) : DEFAULTS.treeWidth,
      detailWidth:
        Number(parsed.detailWidth) > 0 ? Number(parsed.detailWidth) : DEFAULTS.detailWidth,
      detailOpen: parsed.detailOpen !== false,
      columns: parsed.columns && typeof parsed.columns === 'object' ? parsed.columns : {},
      viewMode: parsed.viewMode === 'grid' ? 'grid' : 'list',
    };
  } catch {
    return { ...DEFAULTS, columns: {} };
  }
}

export function saveLayout(p: LayoutPrefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // 存储满/禁用时静默降级
  }
}
