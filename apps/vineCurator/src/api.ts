import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

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

export const probeTools = (): Promise<ToolsReport> => invoke<ToolsReport>('probe_tools');

export const getDbInfo = (): Promise<DbInfo> => invoke<DbInfo>('get_db_info');

export const getSettings = (): Promise<SettingEntry[]> => invoke<SettingEntry[]>('get_settings');

export const saveSettings = (entries: SettingEntry[]): Promise<void> =>
  invoke('save_settings', { entries });

export const listRoots = (): Promise<RootEntry[]> => invoke<RootEntry[]>('list_roots');

export const addRoot = (path: string, label: string | null): Promise<RootEntry> =>
  invoke<RootEntry>('add_root', { path, label });

export const removeRoot = (rootId: number): Promise<void> => invoke('remove_root', { rootId });

export const redirectRoot = (rootId: number, newPath: string): Promise<RootEntry> =>
  invoke<RootEntry>('redirect_root', { rootId, newPath });

export const startScan = (rootId: number): Promise<void> => invoke('start_scan', { rootId });

export const startHashJob = (rootId: number | null): Promise<void> =>
  invoke('start_hash_job', { rootId });

export const startMetaJob = (rootId: number | null, source: string): Promise<void> =>
  invoke('start_meta_job', { rootId, source });

export const cancelJob = (): Promise<void> => invoke('cancel_job');

export const startDupeJob = (): Promise<void> => invoke('start_dupe_job');

export const listDupes = (): Promise<DupeGroup[]> => invoke<DupeGroup[]>('list_dupes');

export const startOcrJob = (rootId: number | null): Promise<void> =>
  invoke('start_ocr_job', { rootId });

export const ocrPendingCount = (rootId: number | null): Promise<number> =>
  invoke<number>('ocr_pending_count', { rootId });

export interface BackupReport {
  snapshot_path: string;
  kept: number;
  restic: string | null;
  restic_skipped: string | null;
}

export const backupNow = (): Promise<BackupReport> => invoke<BackupReport>('backup_now');

export interface ImportReport {
  categories_created: number;
  categories_updated: number;
  tags_created: number;
  aliases_added: number;
  skipped: string[];
}

export const exportPort = (path: string): Promise<number> =>
  invoke<number>('export_port', { path });

export const importPort = (path: string): Promise<ImportReport> =>
  invoke<ImportReport>('import_port', { path });

export const onScanProgress = (cb: (p: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>('scan-progress', (e) => cb(e.payload));

export const onHashProgress = (cb: (p: HashProgress) => void): Promise<UnlistenFn> =>
  listen<HashProgress>('hash-progress', (e) => cb(e.payload));

export const onMetaProgress = (cb: (p: MetaProgress) => void): Promise<UnlistenFn> =>
  listen<MetaProgress>('meta-progress', (e) => cb(e.payload));

export const onRuleProgress = (cb: (p: RuleProgress) => void): Promise<UnlistenFn> =>
  listen<RuleProgress>('rule-progress', (e) => cb(e.payload));

export const onDupeProgress = (cb: (p: DupeProgress) => void): Promise<UnlistenFn> =>
  listen<DupeProgress>('dupe-progress', (e) => cb(e.payload));

export const onOcrProgress = (cb: (p: OcrProgress) => void): Promise<UnlistenFn> =>
  listen<OcrProgress>('ocr-progress', (e) => cb(e.payload));

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
  invoke<ItemPage>('list_items', { filter });

export const listCategories = (): Promise<CategoryNode[]> =>
  invoke<CategoryNode[]>('list_categories');

export const getOverview = (rootId: number | null): Promise<Overview> =>
  invoke<Overview>('get_overview', { rootId });

export const getItem = (itemId: number): Promise<ItemDetail> =>
  invoke<ItemDetail>('get_item', { itemId });

export const searchItems = (query: string, limit: number | null): Promise<ItemRow[]> =>
  invoke<ItemRow[]>('search_items', { query, limit });

export const openPath = (path: string): Promise<void> => invoke('open_path', { path });

export const setItemFields = (
  itemIds: number[],
  fields: { status?: string | null; rating?: number | null; year?: number | null },
): Promise<number> => invoke<number>('set_item_fields', { itemIds, ...fields });

export const revealPath = (path: string): Promise<void> => invoke('reveal_path', { path });

// ---------------------------------------------------------------- 分类树 CRUD

export const createCategory = (parentId: number | null, name: string): Promise<number> =>
  invoke<number>('create_category', { parentId, name });

export const renameCategory = (categoryId: number, name: string): Promise<void> =>
  invoke('rename_category', { categoryId, name });

export const moveCategory = (categoryId: number, parentId: number | null): Promise<void> =>
  invoke('move_category', { categoryId, parentId });

export const deleteCategory = (categoryId: number): Promise<void> =>
  invoke('delete_category', { categoryId });

export const setItemCategories = (itemId: number, categoryIds: number[]): Promise<void> =>
  invoke('set_item_categories', { itemId, categoryIds });

export const addItemCategories = (itemIds: number[], path: string): Promise<number> =>
  invoke<number>('add_item_categories', { itemIds, path });

// ---------------------------------------------------------------- 标签

export interface TagRow {
  id: number;
  name: string;
  item_count: number;
  aliases: string[];
}

export const listTags = (): Promise<TagRow[]> => invoke<TagRow[]>('list_tags');

export const createTag = (name: string): Promise<number> => invoke<number>('create_tag', { name });

export const renameTag = (tagId: number, name: string): Promise<void> =>
  invoke('rename_tag', { tagId, name });

export const addTagAlias = (alias: string, tagId: number): Promise<void> =>
  invoke('add_tag_alias', { alias, tagId });

export const mergeTags = (srcId: number, dstId: number): Promise<number> =>
  invoke<number>('merge_tags', { srcId, dstId });

export const setItemTags = (itemIds: number[], tagNames: string[]): Promise<number> =>
  invoke<number>('set_item_tags', { itemIds, tagNames });

export const removeItemTags = (itemIds: number[], tagIds: number[]): Promise<number> =>
  invoke<number>('remove_item_tags', { itemIds, tagIds });

// ---------------------------------------------------------------- 智能集合

export interface CollectionRow {
  id: number;
  name: string;
  query_json: string;
  sort_order: number;
}

export const listCollections = (): Promise<CollectionRow[]> =>
  invoke<CollectionRow[]>('list_collections');

export const saveCollection = (
  id: number | null,
  name: string,
  queryJson: string,
  sortOrder: number | null,
): Promise<number> => invoke<number>('save_collection', { id, name, queryJson, sortOrder });

export const previewCollection = (
  queryJson: string,
  offset: number | null,
): Promise<[number, ItemRow[]]> =>
  invoke<[number, ItemRow[]]>('preview_collection', { queryJson, offset });

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

export const listRules = (): Promise<RuleRow[]> => invoke<RuleRow[]>('list_rules');

export const saveRule = (
  id: number | null,
  name: string,
  enabled: boolean,
  priority: number,
  conditions: string,
  actions: string,
): Promise<number> =>
  invoke<number>('save_rule', { id, name, enabled, priority, conditions, actions });

export const deleteRule = (id: number): Promise<void> => invoke<void>('delete_rule', { id });

export const startRuleJob = (rootId: number | null): Promise<void> =>
  invoke<void>('start_rule_job', { rootId });

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
  invoke<void>('start_autocat_job', { rootId });

export const onAutocatProgress = (cb: (p: AutoCatProgress) => void): Promise<UnlistenFn> =>
  listen<AutoCatProgress>('autocat-progress', (e) => cb(e.payload));

// ---------------------------------------------------------------- 缩略图

export interface ThumbResult {
  path: string;
  created: boolean;
}

export const ensureThumb = (itemId: number): Promise<ThumbResult> =>
  invoke<ThumbResult>('ensure_thumb', { itemId });

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
  invoke<AuthUser>('setup_admin', { username, password });

export const sessionUser = (): Promise<AuthUser | null> => invoke<AuthUser | null>('session_user');

export const login = (username: string, password: string): Promise<AuthUser> =>
  invoke<AuthUser>('login', { username, password });

export const logout = (): Promise<void> => invoke<void>('logout');

export const changePassword = (oldPassword: string, newPassword: string): Promise<void> =>
  invoke<void>('change_password', { oldPassword, newPassword });

export const listUsers = (): Promise<AuthUser[]> => invoke<AuthUser[]>('list_users');

export const createUser = (username: string, password: string, role: string): Promise<number> =>
  invoke<number>('create_user', { username, password, role });

export const deleteUser = (userId: number): Promise<void> =>
  invoke<void>('delete_user', { userId });

export const setUserRole = (userId: number, role: string): Promise<void> =>
  invoke<void>('set_user_role', { userId, role });

export const listAudit = (limit = 50, entity: string | null = null): Promise<AuditRow[]> =>
  invoke<AuditRow[]>('list_audit', { limit, entity });
