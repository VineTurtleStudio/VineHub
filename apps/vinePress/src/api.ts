import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { hubInvoke, hubTaskRequest, isHubEmbedded } from './host-bridge';

// ---------------------------------------------------------------- 类型（与 Rust 侧 serde 对齐）

export interface UnitStats {
  total_size: number;
  file_count: number;
  image_count: number;
  audio_count: number;
  video_count: number;
  doc_count: number;
  archive_count: number;
  text_count: number;
  subdirs: { name: string; file_count: number; total_size: number }[];
  /** 单位直属内容文件名（不含子目录）：拆分警示用；旧项目文件可能缺省 */
  direct_files?: string[];
  sample_names?: string[];
}

export interface UnitInfo {
  rel_path: string;
  name: string;
  kind: string;
  has_notes: boolean;
  /** 需人工复核（评分层分差不足 / other / 音视频混合内容） */
  needs_review: boolean;
  stats: UnitStats;
}

export interface DetectNode {
  name: string;
  rel_path: string;
  is_unit: boolean;
  kind: string;
  has_notes: boolean;
  /** 需人工复核（评分层分差不足 / other / 音视频混合内容） */
  needs_review: boolean;
  stats: UnitStats | null;
  children: DetectNode[];
}

export interface ScanResponse {
  tree: DetectNode[];
  entries: Entry[];
  container_count: number;
  ignored_count: number;
  total_size: number;
  total_files: number;
  /** 根目录直属内容文件（不入任何单位，仅提示） */
  root_loose: string[];
}

/** 自动识别填充的单条提案 */
export interface Proposal {
  rel_path: string;
  field: string;
  value: string;
  source: string;
  old: string;
}

export interface FileItem {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  mtime_ms: number;
  ext: string;
}

export interface Entry {
  rel_path: string;
  name: string;
  kind: string;
  title: string;
  artist: string;
  date: string;
  label: string;
  catalog: string;
  publisher: string;
  rank: string;
  work: string;
  category: string;
  notes: string;
  tags: string[];
  tracklist: string[];
  password_profile: string | null;
  selected: boolean;
  status: string;
  has_notes: boolean;
  /** 需人工复核（低置信 / 音视频混合），手动改类型后清零 */
  needs_review: boolean;
  stats: UnitStats;
}

export interface PasswordProfile {
  id: string;
  usage: string;
  passwords: string[];
}

export interface ArchiveSettings {
  format: string;
  volume_mib: number;
  comment: boolean;
  encrypt_names: boolean;
  overwrite: boolean;
  write_notes: boolean;
  /** 增量导出：按导出清单指纹跳过源与设置均未变动的单位 */
  incremental: boolean;
}

/** 扫描识别的手动覆盖（并入上级 / 强制为单位 / 强制为容器） */
export interface Overrides {
  force_units: string[];
  force_containers: string[];
}

export function emptyOverrides(): Overrides {
  return { force_units: [], force_containers: [] };
}

export interface Project {
  version: number;
  name: string;
  root: string;
  output_root: string;
  password_profiles: PasswordProfile[];
  archive: ArchiveSettings;
  /** 手动覆盖；旧项目缺省时由 Rust 侧补空数组 */
  overrides: Overrides;
  entries: Entry[];
}

/** 磁盘重命名的结果：新相对路径 + 已同步的条目列表 */
export interface RenameOutcome {
  new_rel_path: string;
  entries: Entry[];
}

/** 命名检查的单条问题（kind = dir | file | root；root 不可自动修） */
export interface NameIssue {
  rel_path: string;
  kind: string;
  name: string;
  /** 建议新名；与 name 相同 = 不可自动修 */
  new_name: string;
  /** error（违规）| warn（提示） */
  level: string;
  reasons: string[];
}

/** 命名检查结果 */
export interface NameCheckResult {
  profile: string;
  issues: NameIssue[];
  checked_dirs: number;
  checked_files: number;
}

/** 批量修复的待改项 */
export interface FixNameItem {
  rel_path: string;
  new_name: string;
}

export interface RenamePair {
  old_rel: string;
  new_rel: string;
}

export interface FixNamesFail {
  rel_path: string;
  message: string;
}

/** 批量修复结果：entries 已同步，接替项目状态后重扫即可 */
export interface FixNamesOutcome {
  entries: Entry[];
  renamed: RenamePair[];
  failed: FixNamesFail[];
}

/** 导出自检问题（error 阻碍成功，warn 仅提示） */
export interface PreflightIssue {
  level: 'error' | 'warn' | string;
  rel_path: string;
  message: string;
}

export interface ToolInfo {
  rar: string | null;
  seven_zip: string | null;
}

export interface NotesProgress {
  phase: string;
  index: number;
  total: number;
  rel_path: string;
  status: string;
  message: string;
  written: number;
  failed: number;
}

/** 单个说明文件的核对结果 */
export interface NoteAuditItem {
  rel_path: string;
  name: string;
  exists: boolean;
  /** 现有文件缺失的字段标签（新模板要求） */
  missing: string[];
  changed: number;
  added: number;
  removed: number;
  same: number;
  error: string | null;
}

export interface AuditProgress {
  phase: string;
  index: number;
  total: number;
  rel_path: string;
  status: string;
  message: string;
  item: NoteAuditItem | null;
}

/** 一组重复文件（相同大小 + 相同 SHA-256） */
export interface DupGroup {
  size: number;
  hash: string;
  /** 相对根目录的路径（正斜杠，字典序，首个为默认保留） */
  files: string[];
}

export interface DupesProgress {
  phase: string;
  index: number;
  total: number;
  rel_path: string;
  status: string;
  message: string;
  groups: DupGroup[];
}

/** 导出清单中单个压缩包的校验结果 */
export interface VerifyItem {
  rel_path: string;
  archive: string;
  /** ok | missing | size | hash | error */
  status: string;
  expected_size: number;
  actual_size: number;
}

/** 完整性校验报告（仅 finished 事件携带） */
export interface VerifyReport {
  total: number;
  ok: number;
  missing: number;
  size_bad: number;
  hash_bad: number;
  error: number;
  unlisted: string[];
  items: VerifyItem[];
}

export interface VerifyProgress {
  /** check | finished */
  phase: string;
  index: number;
  total: number;
  rel_path: string;
  /** running | finished | cancelled | error */
  status: string;
  message: string;
  report: VerifyReport;
}

export interface ExportProgress {
  phase: string;
  index: number;
  total: number;
  rel_path: string;
  unit_percent: number;
  global_percent: number;
  status: string;
  message: string;
  bytes: number;
  created: number;
  skipped: number;
  failed: number;
  archives: string[];
}

export interface ExportRequest {
  root: string;
  output_root: string;
  entries: Entry[];
  settings: ArchiveSettings;
  profiles: PasswordProfile[];
}

// ---------------------------------------------------------------- 命令封装
// P3.3 双模：hub iframe → host-bridge（postMessage）；独立 → 原生 invoke。
// 仅已映射 sidecar RPC 的命令走双模；其余保持独立 invoke（宿主内暂不可用）。

export const scanDirectory = (root: string, overrides: Overrides): Promise<ScanResponse> =>
  isHubEmbedded()
    ? hubTaskRequest<ScanResponse>('scan.directory', { root, overrides })
    : invoke<ScanResponse>('scan_directory', { root, overrides });

export const probeTools = (): Promise<ToolInfo> =>
  isHubEmbedded()
    ? hubInvoke<ToolInfo>('tools.probe', 'probe_tools')
    : invoke<ToolInfo>('probe_tools');

export const saveProject = (path: string, project: Project): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('project.save', 'save_project', { path, project }).then(() => undefined)
    : invoke('save_project', { path, project });

export const loadProject = (path: string): Promise<Project> =>
  isHubEmbedded()
    ? hubInvoke<Project>('project.load', 'load_project', { path })
    : invoke<Project>('load_project', { path });

export const startGenerateNotes = (root: string, entries: Entry[]): Promise<void> =>
  invoke('start_generate_notes', { root, entries });

export const startNoteAudit = (root: string, entries: Entry[]): Promise<void> =>
  invoke('start_note_audit', { root, entries });

export const startFindDuplicates = (root: string): Promise<void> =>
  invoke('start_find_duplicates', { root });

export const startVerify = (outputRoot: string): Promise<void> =>
  invoke('start_verify', { outputRoot });

export const startExport = (request: ExportRequest): Promise<void> =>
  invoke('start_export', { request });

export const cancelJob = (): Promise<void> => invoke('cancel_job');

// ---------------------------------------------------------------- 自动识别填充 / 文件操作

export const autoFillProposals = (
  root: string,
  entries: Entry[],
  fields: string[],
): Promise<Proposal[]> =>
  invoke<Proposal[]>('auto_fill_proposals', { root, entries, fields });

export const listDir = (root: string, path: string, showAll: boolean): Promise<FileItem[]> =>
  isHubEmbedded()
    ? hubInvoke<FileItem[]>('fs.list', 'list_dir', { root, path, showAll })
    : invoke<FileItem[]>('list_dir', { root, path, showAll });

export const openPath = (root: string, path: string): Promise<void> =>
  invoke('open_path', { root, path });

export const revealPath = (root: string, path: string): Promise<void> =>
  invoke('reveal_path', { root, path });

export const makeDirectory = (root: string, path: string): Promise<void> =>
  isHubEmbedded()
    ? hubInvoke('fs.mkdir', 'make_directory', { root, path }).then(() => undefined)
    : invoke('make_directory', { root, path });

export const copyItems = (root: string, items: string[], dest: string): Promise<string[]> =>
  invoke<string[]>('copy_items', { root, items, dest });

export const moveItems = (root: string, items: string[], dest: string): Promise<string[]> =>
  invoke<string[]>('move_items', { root, items, dest });

export const renamePath = (
  root: string,
  entries: Entry[],
  path: string,
  newName: string,
): Promise<RenameOutcome> =>
  invoke<RenameOutcome>('rename_path', { root, entries, path, newName });

// ---------------------------------------------------------------- 命名检查 / 修复

/** 命名档案下拉项 [(id, 显示名)] */
export const nameProfiles = (): Promise<[string, string][]> =>
  isHubEmbedded()
    ? hubInvoke<[string, string][]>('names.profiles', 'name_profiles')
    : invoke<[string, string][]>('name_profiles');

/** 按目标平台/网盘规则全盘检查名称（只读） */
export const checkNames = (root: string, profile: string): Promise<NameCheckResult> =>
  invoke<NameCheckResult>('check_names', { root, profile });

/** 批量修复命名（磁盘重命名 + 条目/说明文件名同步） */
export const fixNames = (
  root: string,
  entries: Entry[],
  items: FixNameItem[],
): Promise<FixNamesOutcome> =>
  invoke<FixNamesOutcome>('fix_names', { root, entries, items });

// ---------------------------------------------------------------- 预览 / 自检 / 回收站

export const previewNote = (root: string, entry: Entry): Promise<string> =>
  isHubEmbedded()
    ? hubInvoke<{ text: string }>('notes.preview', 'preview_note', { root, entry }).then(
        (r) => r.text,
      )
    : invoke<string>('preview_note', { root, entry });

/** 说明文件字段级对比的一行（现有 vs 按当前条目重新渲染） */
export interface FieldDiff {
  field: string;
  old: string;
  new: string;
  status: 'added' | 'removed' | 'changed' | 'same';
}

export interface NoteDiffResult {
  existing: boolean;
  fields: FieldDiff[];
}

export const noteDiff = (root: string, entry: Entry): Promise<NoteDiffResult> =>
  invoke<NoteDiffResult>('note_diff', { root, entry });

/** 单位封面缩略图（data URL）；无封面返回 null */
export const coverImage = (root: string, relPath: string): Promise<string | null> =>
  invoke<string | null>('cover_image', { root, relPath });

export const preflightExport = (
  root: string,
  outputRoot: string,
  entries: Entry[],
  settings: ArchiveSettings,
  profiles: PasswordProfile[],
): Promise<PreflightIssue[]> =>
  invoke<PreflightIssue[]>('preflight_export', {
    root,
    outputRoot,
    entries,
    settings,
    profiles,
  });

export const trashItems = (root: string, items: string[]): Promise<string[]> =>
  invoke<string[]>('trash_items', { root, items });

/** 退出应用（菜单「文件 → 退出」） */
export const quitApp = (): Promise<void> => invoke('quit_app');

// 事件订阅：宿主 iframe 内无 Tauri 事件系统，返回 no-op 保持调用方不崩
// （任务进度在 hub 模式经 task.list 轮询，见 host-bridge.hubTaskRequest）。
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

export const onNotesProgress = (cb: (p: NotesProgress) => void): Promise<UnlistenFn> =>
  safeListen<NotesProgress>('notes-progress', cb);

export const onNoteAuditProgress = (cb: (p: AuditProgress) => void): Promise<UnlistenFn> =>
  safeListen<AuditProgress>('note-audit-progress', cb);

export const onDupesProgress = (cb: (p: DupesProgress) => void): Promise<UnlistenFn> =>
  safeListen<DupesProgress>('dupes-progress', cb);

export const onVerifyProgress = (cb: (p: VerifyProgress) => void): Promise<UnlistenFn> =>
  safeListen<VerifyProgress>('verify-progress', cb);

export const onExportProgress = (cb: (p: ExportProgress) => void): Promise<UnlistenFn> =>
  safeListen<ExportProgress>('export-progress', cb);
