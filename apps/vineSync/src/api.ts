import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, Config, FilterTestDto, LogRow, Preview, StateDto } from "./types";

export const getState = () => invoke<StateDto>("get_state");

export const getAppInfo = () => invoke<AppInfo>("get_app_info");

export const saveConfig = (config: Config) =>
  invoke<StateDto>("save_config", { config });

export const previewSync = (pairId?: string | null) =>
  invoke<Preview>("preview_sync", { request: { pairId: pairId ?? null } });

export const startSync = (pairId?: string | null, trigger = "manual") =>
  invoke<void>("start_sync", {
    request: { pairId: pairId ?? null, trigger },
  });

export const getHistory = (limit?: number) =>
  invoke<LogRow[]>("get_history", { limit: limit ?? null });

export const clearHistory = () => invoke<void>("clear_history");

export const cleanupVersions = (pairId?: string | null) =>
  invoke<number>("cleanup_versions", {
    request: { pairId: pairId ?? null },
  });

export const testFilter = (include: string[], exclude: string[], path: string) =>
  invoke<FilterTestDto>("test_filter", { include, exclude, path });

export const exportHistory = (path: string, rows: LogRow[]) =>
  invoke<number>("export_history", { path, rows });

export const setSyncPaused = (paused: boolean) =>
  invoke<void>("set_sync_paused", { paused });

export function newPairId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}
