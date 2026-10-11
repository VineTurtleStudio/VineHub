// 与 Rust 后端 (src-tauri/src/model.rs) 对应的类型定义

export type DeletePolicy = "versioned" | "mirror" | "addOnly";
export type TriggerMode = "manual" | "interval" | "realtime";
export type Theme = "system" | "dark" | "light";
export type ChangeKind = "add" | "update" | "delete" | "versioned";
export type Language = "zh" | "en";
export type AfterSyncAction = "none" | "script" | "shutdown";

export interface TriggerConfig {
  mode: TriggerMode;
  intervalMinutes: number;
  debounceMs: number;
}

export interface PairConfig {
  id: string;
  name: string;
  source: string;
  dest: string;
  enabled: boolean;
  include: string[];
  exclude: string[];
  /** 定时同步间隔覆盖（分钟，null = 使用全局间隔） */
  intervalMinutes: number | null;
}

export interface Config {
  pairs: PairConfig[];
  deletePolicy: DeletePolicy;
  trigger: TriggerConfig;
  hashVerify: boolean;
  notifyOnFinish: boolean;
  confirmBeforeSync: boolean;
  versionKeepDays: number;
  versionOnReplace: boolean;
  versionOnDelete: boolean;
  versionMaxCount: number;
  copyThreads: number;
  theme: Theme;
  language: Language;
  autostart: boolean;
  afterSyncAction: AfterSyncAction;
  afterSyncScript: string;
  versionTimestampFormat: string;
  verifyAfterCopy: boolean;
  diffScan: boolean;
}

export interface PairState {
  pairId: string;
  lastSyncAt: string | null;
  lastStatus: string | null;
  added: number;
  updated: number;
  deleted: number;
  versioned: number;
  failed: number;
  message: string;
}

export interface StateDto {
  config: Config;
  pairStates: PairState[];
  syncing: boolean;
  queued: boolean;
}

export interface Counts {
  added: number;
  updated: number;
  deleted: number;
  versioned: number;
  skipped: number;
  failed: number;
}

export interface Change {
  pairId: string;
  pairName: string;
  kind: ChangeKind;
  relPath: string;
  size: number;
}

export interface Preview {
  changes: Change[];
  counts: Counts;
  totalBytes: number;
  suppressed: number;
  targetUnavailable: boolean;
  errors: string[];
}

export interface LogRow {
  id: number;
  pairId: string;
  pairName: string;
  startedAt: string;
  finishedAt: string;
  status: string;
  added: number;
  updated: number;
  deleted: number;
  versioned: number;
  skipped: number;
  failed: number;
  trigger: string;
  message: string;
}

export interface StartedEvt {
  trigger: string;
  pairs: number;
}

export interface ProgressEvt {
  pairName: string;
  relPath: string;
  action: ChangeKind;
  done: number;
  total: number;
  /** 已完成变更的累计传输字节数（仅新增/更新计入） */
  bytes: number;
}

/** 过滤器测试结果 */
export interface FilterTestDto {
  matched: boolean;
  reason: string;
}

export interface FinishedEvt {
  ok: boolean;
  trigger: string;
  pairs: number;
  counts: Counts;
  failures: number;
  targetUnavailable: boolean;
  durationMs: number;
  message: string;
}

/** 扫描进度（预览与同步的目录扫描阶段） */
export interface ScanProgressEvt {
  pairName: string;
  phase: "source" | "target";
  files: number;
  relPath: string;
}

/** 应用信息（便携标识、数据目录等） */
export interface AppInfo {
  portable: boolean;
  dataDir: string;
  exePath: string;
  version: string;
}
