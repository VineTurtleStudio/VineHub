export interface PluginView {
  id: string;
  name: string;
  version: string;
  apiVersion: string;
  kind: "external" | "sidecar";
  enabled: boolean;
  available: boolean;
  statusReason: string | null;
  entryOverride: string | null;
  resolvedEntry: string | null;
}

export interface ImportResult {
  ok: boolean;
  message: string;
  pluginId: string | null;
}

export interface ScanStartResult {
  started: boolean;
}

export interface ProgressNote {
  taskId: string;
  done: number;
  total: number;
  msg?: string | null;
}

export interface ScanPoll {
  notifications: ProgressNote[];
  taskId: string | null;
  terminal: ScanTerminal | null;
  error: string | null;
  running: boolean;
  sidecarRunning: boolean;
}

export interface ScanFinding {
  severity: "ok" | "warning" | "corrupt" | "unknown";
  code: string;
  message: string;
  offset?: number | null;
}

export interface ScanReport {
  path: string;
  file_type: string;
  mode: string;
  findings: ScanFinding[];
  duration?: number;
}

export interface ScanTerminal {
  taskId: string;
  ok: number;
  bad: number;
  warning: number;
  corrupt: number;
  unknown: number;
  scanned: number;
  discovered: number;
  cancelled: boolean;
  elapsedMs: number;
  reports: ScanReport[];
}

export type TaskStatus = "pending" | "running" | "done" | "failed" | "cancelled";

export interface TaskProgress {
  taskId: string;
  done: number;
  total: number;
  msg?: string | null;
}

export interface TaskView {
  id: string;
  pluginId: string;
  method: string;
  status: TaskStatus;
  createdAt: number;
  updatedAt: number;
  detail: string | null;
  progress: TaskProgress | null;
  pluginTaskId: string | null;
}
