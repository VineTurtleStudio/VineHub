import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
  bridgeRequest,
  hubInvoke,
  hubPluginId,
  isHubEmbedded,
} from './host-bridge';

// ---------------------------------------------------------------- 工具

/** 宿主 iframe 内无 Tauri 事件系统，返回 no-op 保持调用方不崩。 */
const safeListen = <T>(
  event: string,
  cb: (payload: T) => void,
): Promise<UnlistenFn> => {
  if (isHubEmbedded()) {
    void event;
    void cb;
    return Promise.resolve(() => {});
  }
  return listen<T>(event, (e) => cb(e.payload));
};

// ---------------------------------------------------------------- 类型（与 Rust 侧 serde 对齐）

export interface ToolStatus {
  name: string;
  found: boolean;
  version: string | null;
  path: string | null;
}

export interface ToolsReport {
  tools: ToolStatus[];
}

export interface SettingEntry {
  key: string;
  value: string;
}

export interface RootEntry {
  id: number;
  path: string;
  label: string;
  item_count: number;
  created_at: string;
}

export interface DbInfo {
  path: string;
}

export interface ScanStats {
  seen: number;
  changed: number;
  unchanged: number;
  missing: number;
  errors: number;
}

export interface ScanProgress {
  phase: string; // walk | finalize | finished
  index: number;
  total: number;
  rel_path: string;
  status: string; // running | done | cancelled | error
  message: string;
  stats: ScanStats;
}

export interface HashStats {
  done: number;
  failed: number;
}

export interface HashProgress {
  phase: string; // hash | finished
  index: number;
  total: number;
  item_id: number;
  rel_path: string;
  status: string; // running | done | cancelled | error
  message: string;
  stats: HashStats;
}

export interface MetaStats {
  probed: number;
  skipped: number;
  failed: number;
  imported: number;
}

export interface DupeProgress {
  phase: string; // hash | finished
  index: number;
  total: number;
  item_id: number;
  rel_path: string;
  status: string; // running | done | cancelled | error
  message: string;
  hashed: number;
  failed: number;
}

export interface DupeItem {
  id: number;
  root_id: number;
  rel_path: string;
  hash_state: string;
  full_hash: string | null;
}

export interface DupeGroup {
  size: number;
  count: number;
  partial_hash: string;
  confirmed: boolean;
  full_hash: string | null;
  items: DupeItem[];
}

export interface OcrProgress {
  phase: string; // queue | finished
  index: number;
  total: number;
  item_id: number;
  rel_path: string;
  status: string; // running | done | cancelled | error
  message: string;
  done: number;
  failed: number;
}

export interface MetaProgress {
  phase: string; // probe | import | finished
  index: number;
  total: number;
  rel_path: string;
  status: string; // running | done | cancelled | error
  message: string;
  stats: MetaStats;
}

// ---------------------------------------------------------------- 环境 / 设置 / 数据库

export const probeTools = (): Promise<ToolsReport> =>
  isHubEmbedded()
    ? hubInvoke<ToolsReport>(null, 'probe_tools')
    : invoke<ToolsReport>('probe_tools');

export const getDbInfo = (): Promise<DbInfo> =>
  isHubEmbedded()
    ? hubInvoke<DbInfo>('db.info', 'get_db_info')
    : invoke<DbInfo>('get_db_info');

export const getSettings = (): Promise<SettingEntry[]> =>
  isHubEmbedded()
    ? hubInvoke<SettingEntry[]>('settings.get', 'get_settings')
    : invoke<SettingEntry[]>('get_settings');

export const saveSettings = (entries: SettingEntry[]): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('settings.save', 'save_settings', { entries }).then(() => undefined)
    : invoke('save_settings', { entries });

// ---------------------------------------------------------------- 根目录

/** RPC library.addRoot / roots.redirect 返回 camelCase root_json，映射为 snake_case。 */
interface RootJson {
  id: number;
  path: string;
  label: string;
  itemCount: number;
  createdAt: string;
}

const mapRoot = (r: RootJson): RootEntry => ({
  id: r.id,
  path: r.path,
  label: r.label,
  item_count: r.itemCount,
  created_at: r.createdAt,
});

export const listRoots = (): Promise<RootEntry[]> =>
  isHubEmbedded()
    ? hubInvoke<{ roots: RootJson[] }>('library.roots', 'list_roots').then((r) =>
        r.roots.map(mapRoot),
      )
    : invoke<RootEntry[]>('list_roots');

export const addRoot = (path: string, label: string | null): Promise<RootEntry> =>
  isHubEmbedded()
    ? hubInvoke<RootJson>('library.addRoot', 'add_root', { path, label }).then(mapRoot)
    : invoke<RootEntry>('add_root', { path, label });

export const removeRoot = (rootId: number): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('roots.remove', 'remove_root', { rootId }).then(() => undefined)
    : invoke('remove_root', { rootId });

export const redirectRoot = (rootId: number, newPath: string): Promise<RootEntry> =>
  isHubEmbedded()
    ? hubInvoke<RootJson>('roots.redirect', 'redirect_root', { rootId, newPath }).then(mapRoot)
    : invoke<RootEntry>('redirect_root', { rootId, newPath });

// ---------------------------------------------------------------- 后台任务（fire-and-forget）

export const startScan = (rootId: number): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('library.scan', 'start_scan', { rootId }).then(() => undefined)
    : invoke('start_scan', { rootId });

export const startHashJob = (rootId: number | null): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('hash.start', 'start_hash_job', { rootId }).then(() => undefined)
    : invoke('start_hash_job', { rootId });

export const startMetaJob = (rootId: number | null, source: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('meta.start', 'start_meta_job', { rootId, source }).then(() => undefined)
    : invoke('start_meta_job', { rootId, source });

export const cancelJob = async (): Promise<void> => {
  if (!isHubEmbedded()) {
    return invoke('cancel_job');
  }
  const pluginId = hubPluginId();
  if (!pluginId) return;
  const { tasks } = await bridgeRequest<{
    tasks: { id: string; status: string }[];
  }>('task.list', { pluginId });
  const running = tasks.find((t) => t.status === 'running');
  if (running) {
    await bridgeRequest('task.cancel', { taskId: running.id });
  }
};

export const startDupeJob = (): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('dupe.start', 'start_dupe_job').then(() => undefined)
    : invoke('start_dupe_job');

export const listDupes = (): Promise<DupeGroup[]> =>
  isHubEmbedded()
    ? hubInvoke<DupeGroup[]>('dupes.list', 'list_dupes')
    : invoke<DupeGroup[]>('list_dupes');

export const startOcrJob = (rootId: number | null): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('ocr.start', 'start_ocr_job', { rootId }).then(() => undefined)
    : invoke('start_ocr_job', { rootId });

export const ocrPendingCount = (rootId: number | null): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ pending: number }>('ocr.pending', 'ocr_pending_count', { rootId }).then(
        (r) => r.pending,
      )
    : invoke<number>('ocr_pending_count', { rootId });

// ---------------------------------------------------------------- 备份 / 导入导出

export interface BackupReport {
  snapshot_path: string;
  kept: number;
  restic: string | null;
  restic_skipped: string | null;
}

export const backupNow = (): Promise<BackupReport> =>
  isHubEmbedded()
    ? hubInvoke<BackupReport>('backup.now', 'backup_now')
    : invoke<BackupReport>('backup_now');

export interface ImportReport {
  categories_created: number;
  categories_updated: number;
  tags_created: number;
  aliases_added: number;
  skipped: string[];
}

export const exportPort = (path: string): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ count: number }>('port.export', 'export_port', { path }).then((r) => r.count)
    : invoke<number>('export_port', { path });

export const importPort = (path: string): Promise<ImportReport> =>
  isHubEmbedded()
    ? hubInvoke<ImportReport>('port.import', 'import_port', { path })
    : invoke<ImportReport>('import_port', { path });

// ---------------------------------------------------------------- 进度事件（宿主模式 no-op）

export const onScanProgress = (cb: (p: ScanProgress) => void): Promise<UnlistenFn> =>
  safeListen<ScanProgress>('scan-progress', cb);

export const onHashProgress = (cb: (p: HashProgress) => void): Promise<UnlistenFn> =>
  safeListen<HashProgress>('hash-progress', cb);

export const onMetaProgress = (cb: (p: MetaProgress) => void): Promise<UnlistenFn> =>
  safeListen<MetaProgress>('meta-progress', cb);

export const onRuleProgress = (cb: (p: RuleProgress) => void): Promise<UnlistenFn> =>
  safeListen<RuleProgress>('rule-progress', cb);

export const onDupeProgress = (cb: (p: DupeProgress) => void): Promise<UnlistenFn> =>
  safeListen<DupeProgress>('dupe-progress', cb);

export const onOcrProgress = (cb: (p: OcrProgress) => void): Promise<UnlistenFn> =>
  safeListen<OcrProgress>('ocr-progress', cb);

// ---------------------------------------------------------------- 浏览

export interface ItemRow {
  id: number;
  root_id: number;
  rel_path: string;
  file_name: string;
  ext: string;
  type_name: string;
  size: number;
  mtime: number;
  status: string;
  rating: number;
  year: number | null;
  missing: boolean;
  hash_state: string;
  meta_state: string;
}

/** RPC search.items 返回 camelCase item_json，映射为 snake_case。 */
interface ItemJson {
  id: number;
  rootId: number;
  relPath: string;
  fileName: string;
  ext: string;
  typeName: string;
  size: number;
  mtime: number;
  status: string;
  rating: number;
  year: number | null;
  missing: boolean;
  hashState: string;
  metaState: string;
}

const mapItem = (i: ItemJson): ItemRow => ({
  id: i.id,
  root_id: i.rootId,
  rel_path: i.relPath,
  file_name: i.fileName,
  ext: i.ext,
  type_name: i.typeName,
  size: i.size,
  mtime: i.mtime,
  status: i.status,
  rating: i.rating,
  year: i.year,
  missing: i.missing,
  hash_state: i.hashState,
  meta_state: i.metaState,
});

export interface ItemFilter {
  root_id?: number | null;
  type_name?: string | null;
  status?: string | null;
  category_id?: number | null;
  offset?: number | null;
  limit?: number | null;
  sort?: string | null;
  desc?: boolean | null;
}

export interface ItemPage {
  total: number;
  items: ItemRow[];
}

export interface CategoryNode {
  id: number;
  parent_id: number | null;
  name: string;
  path: string;
  item_count: number;
  children: CategoryNode[];
}

export interface CountEntry {
  key: string;
  count: number;
}

export interface Overview {
  total: number;
  missing: number;
  by_type: CountEntry[];
  by_status: CountEntry[];
}

export interface ExtField {
  name: string;
  value: string | null;
}

export interface TagEntry {
  id: number;
  name: string;
  source: string;
}

export interface ItemDetail {
  item: ItemRow;
  root_path: string;
  ext: ExtField[];
  tags: TagEntry[];
  category_ids: number[];
  metadata: string;
}

export const listItems = (filter: ItemFilter): Promise<ItemPage> =>
  isHubEmbedded()
    ? hubInvoke<ItemPage>('items.list', 'list_items', { filter })
    : invoke<ItemPage>('list_items', { filter });

export const listCategories = (): Promise<CategoryNode[]> =>
  isHubEmbedded()
    ? hubInvoke<CategoryNode[]>('categories.list', 'list_categories')
    : invoke<CategoryNode[]>('list_categories');

export const getOverview = (rootId: number | null): Promise<Overview> =>
  isHubEmbedded()
    ? hubInvoke<Overview>('overview.get', 'get_overview', { rootId })
    : invoke<Overview>('get_overview', { rootId });

export const getItem = (itemId: number): Promise<ItemDetail> =>
  isHubEmbedded()
    ? hubInvoke<ItemDetail>('item.get', 'get_item', { itemId })
    : invoke<ItemDetail>('get_item', { itemId });

export const searchItems = (query: string, limit: number | null): Promise<ItemRow[]> =>
  isHubEmbedded()
    ? hubInvoke<{ total: number; items: ItemJson[] }>('search.items', 'search_items', {
        query,
        limit,
      }).then((r) => r.items.map(mapItem))
    : invoke<ItemRow[]>('search_items', { query, limit });

export const openPath = (path: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('path.open', 'open_path', { path }).then(() => undefined)
    : invoke('open_path', { path });

export const setItemFields = (
  itemIds: number[],
  fields: { status?: string | null; rating?: number | null; year?: number | null },
): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ updated: number }>('item.setFields', 'set_item_fields', {
        itemIds,
        ...fields,
      }).then((r) => r.updated)
    : invoke<number>('set_item_fields', { itemIds, ...fields });

export const revealPath = (path: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('path.reveal', 'reveal_path', { path }).then(() => undefined)
    : invoke('reveal_path', { path });

// ---------------------------------------------------------------- 分类树 CRUD

export const createCategory = (parentId: number | null, name: string): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ id: number }>('category.create', 'create_category', { parentId, name }).then(
        (r) => r.id,
      )
    : invoke<number>('create_category', { parentId, name });

export const renameCategory = (categoryId: number, name: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('category.rename', 'rename_category', { categoryId, name }).then(() => undefined)
    : invoke('rename_category', { categoryId, name });

export const moveCategory = (categoryId: number, parentId: number | null): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('category.move', 'move_category', { categoryId, parentId }).then(() => undefined)
    : invoke('move_category', { categoryId, parentId });

export const deleteCategory = (categoryId: number): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('category.delete', 'delete_category', { categoryId }).then(() => undefined)
    : invoke('delete_category', { categoryId });

export const setItemCategories = (itemId: number, categoryIds: number[]): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('item.categories.set', 'set_item_categories', { itemId, categoryIds }).then(
        () => undefined,
      )
    : invoke('set_item_categories', { itemId, categoryIds });

export const addItemCategories = (itemIds: number[], path: string): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ linked: number }>('item.categories.add', 'add_item_categories', {
        itemIds,
        path,
      }).then((r) => r.linked)
    : invoke<number>('add_item_categories', { itemIds, path });

// ---------------------------------------------------------------- 标签

export interface TagRow {
  id: number;
  name: string;
  item_count: number;
  aliases: string[];
}

export const listTags = (): Promise<TagRow[]> =>
  isHubEmbedded()
    ? hubInvoke<TagRow[]>('tags.list', 'list_tags')
    : invoke<TagRow[]>('list_tags');

export const createTag = (name: string): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ id: number }>('tag.create', 'create_tag', { name }).then((r) => r.id)
    : invoke<number>('create_tag', { name });

export const renameTag = (tagId: number, name: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('tag.rename', 'rename_tag', { tagId, name }).then(() => undefined)
    : invoke('rename_tag', { tagId, name });

export const addTagAlias = (alias: string, tagId: number): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('tag.alias.add', 'add_tag_alias', { alias, tagId }).then(() => undefined)
    : invoke('add_tag_alias', { alias, tagId });

export const mergeTags = (srcId: number, dstId: number): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ moved: number }>('tags.merge', 'merge_tags', { srcId, dstId }).then(
        (r) => r.moved,
      )
    : invoke<number>('merge_tags', { srcId, dstId });

export const setItemTags = (itemIds: number[], tagNames: string[]): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ synced: number }>('item.tags.set', 'set_item_tags', { itemIds, tagNames }).then(
        (r) => r.synced,
      )
    : invoke<number>('set_item_tags', { itemIds, tagNames });

export const removeItemTags = (itemIds: number[], tagIds: number[]): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ synced: number }>('item.tags.remove', 'remove_item_tags', {
        itemIds,
        tagIds,
      }).then((r) => r.synced)
    : invoke<number>('remove_item_tags', { itemIds, tagIds });

// ---------------------------------------------------------------- 智能集合

export interface CollectionRow {
  id: number;
  name: string;
  query_json: string;
  sort_order: number;
}

export const listCollections = (): Promise<CollectionRow[]> =>
  isHubEmbedded()
    ? hubInvoke<CollectionRow[]>('collections.list', 'list_collections')
    : invoke<CollectionRow[]>('list_collections');

export const saveCollection = (
  id: number | null,
  name: string,
  queryJson: string,
  sortOrder: number | null,
): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ id: number }>('collection.save', 'save_collection', {
        id,
        name,
        queryJson,
        sortOrder,
      }).then((r) => r.id)
    : invoke<number>('save_collection', { id, name, queryJson, sortOrder });

export const previewCollection = (
  queryJson: string,
  offset: number | null,
): Promise<[number, ItemRow[]]> =>
  isHubEmbedded()
    ? hubInvoke<ItemPage>('collection.preview', 'preview_collection', { queryJson, offset }).then(
        (r) => [r.total, r.items] as [number, ItemRow[]],
      )
    : invoke<[number, ItemRow[]]>('preview_collection', { queryJson, offset });

// ---------------------------------------------------------------- 规则自动打标

export interface RuleRow {
  id: number;
  name: string;
  enabled: boolean;
  priority: number;
  conditions: string;
  actions: string;
}

export interface RuleStats {
  rules_run: number;
  matched: number;
  tagged: number;
  categorized: number;
}

export interface RuleProgress {
  phase: string; // rule | finished
  index: number;
  total: number;
  rule_id: number;
  rule_name: string;
  status: string; // running | done | cancelled | error
  message: string;
  stats: RuleStats;
}

export const listRules = (): Promise<RuleRow[]> =>
  isHubEmbedded()
    ? hubInvoke<RuleRow[]>('rules.list', 'list_rules')
    : invoke<RuleRow[]>('list_rules');

export const saveRule = (
  id: number | null,
  name: string,
  enabled: boolean,
  priority: number,
  conditions: string,
  actions: string,
): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ id: number }>('rule.save', 'save_rule', {
        id,
        name,
        enabled,
        priority,
        conditions,
        actions,
      }).then((r) => r.id)
    : invoke<number>('save_rule', { id, name, enabled, priority, conditions, actions });

export const deleteRule = (id: number): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('rule.delete', 'delete_rule', { id }).then(() => undefined)
    : invoke<void>('delete_rule', { id });

export const startRuleJob = (rootId: number | null): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('rule.start', 'start_rule_job', { rootId }).then(() => undefined)
    : invoke<void>('start_rule_job', { rootId });

// ---------------------------------------------------------------- 自动分类

export interface AutoCatStats {
  items: number;
  linked: number;
  unlinked: number;
}

export interface AutoCatProgress {
  phase: string; // queue | finished
  status: string; // running | done | cancelled | error
  message: string;
  stats: AutoCatStats;
}

export const startAutocatJob = (rootId: number | null): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('autocat.start', 'start_autocat_job', { rootId }).then(() => undefined)
    : invoke<void>('start_autocat_job', { rootId });

export const onAutocatProgress = (cb: (p: AutoCatProgress) => void): Promise<UnlistenFn> =>
  safeListen<AutoCatProgress>('autocat-progress', cb);

// ---------------------------------------------------------------- 缩略图

export interface ThumbResult {
  path: string;
  created: boolean;
}

export const ensureThumb = (itemId: number): Promise<ThumbResult> =>
  isHubEmbedded()
    ? hubInvoke<ThumbResult>('thumb.ensure', 'ensure_thumb', { itemId })
    : invoke<ThumbResult>('ensure_thumb', { itemId });

// ---------------------------------------------------------------- 用户 / 登录 / 审计

export interface AuthUser {
  id: number;
  username: string;
  role: string;
  created_at: string;
}

export interface AuditRow {
  id: number;
  user_id: number | null;
  username: string | null;
  action: string;
  entity: string;
  entity_id: string | null;
  detail: string;
  created_at: string;
}

export const setupAdmin = (username: string, password: string): Promise<AuthUser> =>
  isHubEmbedded()
    ? hubInvoke<AuthUser>('admin.setup', 'setup_admin', { username, password })
    : invoke<AuthUser>('setup_admin', { username, password });

export const sessionUser = (): Promise<AuthUser | null> =>
  isHubEmbedded()
    ? hubInvoke<AuthUser | null>('session.get', 'session_user')
    : invoke<AuthUser | null>('session_user');

export const login = (username: string, password: string): Promise<AuthUser> =>
  isHubEmbedded()
    ? hubInvoke<AuthUser>('auth.login', 'login', { username, password })
    : invoke<AuthUser>('login', { username, password });

export const logout = (): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('auth.logout', 'logout').then(() => undefined)
    : invoke<void>('logout');

export const changePassword = (oldPassword: string, newPassword: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('auth.password.change', 'change_password', { oldPassword, newPassword }).then(
        () => undefined,
      )
    : invoke<void>('change_password', { oldPassword, newPassword });

export const listUsers = (): Promise<AuthUser[]> =>
  isHubEmbedded()
    ? hubInvoke<AuthUser[]>('users.list', 'list_users')
    : invoke<AuthUser[]>('list_users');

export const createUser = (username: string, password: string, role: string): Promise<number> =>
  isHubEmbedded()
    ? hubInvoke<{ id: number }>('user.create', 'create_user', { username, password, role }).then(
        (r) => r.id,
      )
    : invoke<number>('create_user', { username, password, role });

export const deleteUser = (userId: number): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('user.delete', 'delete_user', { userId }).then(() => undefined)
    : invoke<void>('delete_user', { userId });

export const setUserRole = (userId: number, role: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('user.role.set', 'set_user_role', { userId, role }).then(() => undefined)
    : invoke<void>('set_user_role', { userId, role });

export const listAudit = (limit = 50, entity: string | null = null): Promise<AuditRow[]> =>
  isHubEmbedded()
    ? hubInvoke<AuditRow[]>('audit.list', 'list_audit', { limit, entity })
    : invoke<AuditRow[]>('list_audit', { limit, entity });
