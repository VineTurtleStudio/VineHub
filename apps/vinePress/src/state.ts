import type { DetectNode, Entry, FileItem, Overrides, Project, ScanResponse } from './api';
import { emptyOverrides } from './api';

export interface Filters {
  entryQuery: string;
  kind: string;
  status: string;
  treeQuery: string;
  /** 高级筛选：年份（date 前 4 位）、标签、密码方案（方案 id / `__none__` = 不加密），'' = 不限 */
  year: string;
  tag: string;
  profile: string;
  /** 大小区间下/上限，单位 GiB，'' = 不限 */
  sizeMin: string;
  sizeMax: string;
}

export interface SortState {
  key: 'rel_path' | 'title' | 'kind' | 'size' | 'files' | 'status';
  dir: 1 | -1;
}

export interface AppState {
  project: Project | null;
  projectPath: string | null;
  tree: DetectNode[];
  scan: Omit<ScanResponse, 'tree' | 'entries'> | null;
  selectedRel: string | null;
  /** 目录树中选中的节点（容器或单位），用于手动覆盖操作 */
  selectedNodeRel: string | null;
  filters: Filters;
  sort: SortState;
  expanded: Set<string>;
  activeProfileId: string | null;
  /** 文件面板当前目录（绝对路径） */
  cwd: string | null;
  fileItems: FileItem[];
  fileSelection: Set<string>;
  showAllFiles: boolean;
  busy: 'notes' | 'export' | 'scan' | null;
  /** 有未保存的项目修改（指示圆点）；关键路径（退出/新建/打开）另做快照复核 */
  dirty: boolean;
}

export function newState(): AppState {
  return {
    project: null,
    projectPath: null,
    tree: [],
    scan: null,
    selectedRel: null,
    selectedNodeRel: null,
    filters: { entryQuery: '', kind: '', status: '', treeQuery: '', year: '', tag: '', profile: '', sizeMin: '', sizeMax: '' },
    sort: { key: 'rel_path', dir: 1 },
    expanded: new Set<string>(),
    activeProfileId: null,
    cwd: null,
    fileItems: [],
    fileSelection: new Set<string>(),
    showAllFiles: false,
    busy: null,
    dirty: false,
  };
}

export function emptyProject(name = '未命名项目'): Project {
  return {
    version: 1,
    name,
    root: '',
    output_root: '',
    password_profiles: [],
    archive: {
      format: 'rar',
      volume_mib: 900,
      comment: true,
      encrypt_names: true,
      overwrite: false,
      write_notes: true,
      incremental: false,
    },
    overrides: emptyOverrides(),
    entries: [],
  };
}

export function formatSize(bytes: number): string {
  if (!bytes) return '0';
  const mib = 1024 * 1024;
  const gib = 1024 * mib;
  if (bytes >= gib) return `${(bytes / gib).toFixed(2)} GiB`;
  if (bytes >= mib) return `${(bytes / mib).toFixed(2)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

/** 类型全集（显示顺序，值 ↔ 标签）。新增类型只在此登记：类型下拉、批量编辑、徽标、筛选随之生效 */
export const KINDS: [string, string][] = [
  ['album', '专辑'],
  ['novel', '小说'],
  ['artbook', '图册'],
  ['comic', '漫画'],
  ['score', '乐谱'],
  ['concert', '演唱会'],
  ['video', '视频'],
  ['document', '资料'],
  ['other', '其它'],
];

export function kindLabel(kind: string): string {
  return KINDS.find(([v]) => v === kind)?.[1] ?? '其它';
}

export function statusLabel(status: string): string {
  return status === 'done'
    ? '已导出'
    : status === 'error'
      ? '失败'
      : status === 'skipped'
        ? '已存在'
        : '待导出';
}

export function matchesEntry(entry: Entry, f: Filters): boolean {
  if (f.kind && entry.kind !== f.kind) return false;
  if (f.status && entry.status !== f.status) return false;
  if (f.year && !entry.date.trim().startsWith(f.year)) return false;
  if (f.tag && !entry.tags.some((t) => t === f.tag)) return false;
  if (f.profile === '__none__') {
    if (entry.password_profile) return false;
  } else if (f.profile && entry.password_profile !== f.profile) {
    return false;
  }
  const gib = 1024 * 1024 * 1024;
  if (f.sizeMin !== '' && entry.stats.total_size < Number(f.sizeMin) * gib) return false;
  if (f.sizeMax !== '' && entry.stats.total_size > Number(f.sizeMax) * gib) return false;
  const q = f.entryQuery.trim().toLowerCase();
  if (q) {
    const hay = `${entry.title}\n${entry.rel_path}\n${entry.artist}\n${entry.work}\n${entry.category}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  return true;
}

/** 全局搜索用：条目全部可检索元数据字段（小写拼接） */
export function searchHaystack(entry: Entry): string {
  return [
    entry.title,
    entry.name,
    entry.rel_path,
    entry.artist,
    entry.work,
    entry.category,
    kindLabel(entry.kind),
    entry.needs_review ? '复核' : '',
    entry.notes,
    entry.catalog,
    entry.date,
    entry.label,
    entry.publisher,
    entry.rank,
    entry.tags.join(' '),
    entry.tracklist.join(' '),
  ]
    .join('\n')
    .toLowerCase();
}

/** 过滤器是否全部为默认值 */
export function filtersActive(f: Filters): boolean {
  return !!(
    f.entryQuery.trim() ||
    f.kind ||
    f.status ||
    f.year ||
    f.tag ||
    f.profile ||
    f.sizeMin !== '' ||
    f.sizeMax !== ''
  );
}

/** 全部过滤条件归零（高级筛选 + 基础筛选；不含 treeQuery） */
export function clearEntryFilters(f: Filters): void {
  f.entryQuery = '';
  f.kind = '';
  f.status = '';
  f.year = '';
  f.tag = '';
  f.profile = '';
  f.sizeMin = '';
  f.sizeMax = '';
}

export function matchesNode(node: DetectNode, q: string): boolean {
  if (!q) return true;
  const needle = q.toLowerCase();
  if (node.rel_path.toLowerCase().includes(needle)) return true;
  if (node.is_unit) return false;
  return node.children.some((c) => matchesNode(c, q));
}

export function selectedEntries(project: Project | null): Entry[] {
  if (!project) return [];
  return project.entries.filter((e) => e.selected);
}

export function sortEntries(entries: Entry[], sort: SortState): Entry[] {
  const mul = sort.dir;
  const val = (e: Entry): string | number => {
    switch (sort.key) {
      case 'title':
        return (e.title || e.name).toLowerCase();
      case 'kind':
        return e.kind;
      case 'size':
        return e.stats.total_size;
      case 'files':
        return e.stats.file_count;
      case 'status':
        return e.status;
      default:
        return e.rel_path.toLowerCase();
    }
  };
  return [...entries].sort((a, b) => {
    const x = val(a);
    const y = val(b);
    if (typeof x === 'number' && typeof y === 'number') return (x - y) * mul;
    return String(x).localeCompare(String(y), 'zh-Hans-CN') * mul;
  });
}

export function findEntry(project: Project | null, rel: string | null): Entry | null {
  if (!project || !rel) return null;
  return project.entries.find((e) => e.rel_path === rel) ?? null;
}

/** 递归查找目录树节点 */
export function findNode(nodes: DetectNode[], rel: string | null): DetectNode | null {
  if (!rel) return null;
  for (const n of nodes) {
    if (n.rel_path === rel) return n;
    const hit = findNode(n.children, rel);
    if (hit) return hit;
  }
  return null;
}

/** 保证 overrides 字段存在（旧项目 / 外部编辑的 JSON） */
export function ensureOverrides(project: Project): Overrides {
  if (!project.overrides) project.overrides = emptyOverrides();
  if (!Array.isArray(project.overrides.force_units)) project.overrides.force_units = [];
  if (!Array.isArray(project.overrides.force_containers)) project.overrides.force_containers = [];
  return project.overrides;
}

/** 该节点当前的手动覆盖类型 */
export function overrideKind(project: Project | null, rel: string | null): 'unit' | 'container' | null {
  if (!project || !rel || !project.overrides) return null;
  if (project.overrides.force_units.includes(rel)) return 'unit';
  if (project.overrides.force_containers.includes(rel)) return 'container';
  return null;
}

/** 收集某前缀之下的全部单位相对路径 */
export function unitsUnder(nodes: DetectNode[], prefix: string): string[] {
  const out: string[] = [];
  const walk = (list: DetectNode[]): void => {
    for (const n of list) {
      if (n.is_unit && (n.rel_path === prefix || n.rel_path.startsWith(`${prefix}/`))) out.push(n.rel_path);
      walk(n.children);
    }
  };
  walk(nodes);
  return out;
}
