import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, Config, FilterTestDto, LogRow, Preview, StateDto } from "./types";
import { hubInvoke, hubTaskRequest, isHubEmbedded } from "./host-bridge";

// P3.3 双模：hub iframe → host-bridge（postMessage）；独立 → 原生 invoke。

export const getState = () =>
  isHubEmbedded()
    ? hubInvoke<StateDto>("state.get", "get_state")
    : invoke<StateDto>("get_state");

export const getAppInfo = () =>
  isHubEmbedded()
    ? hubInvoke<AppInfo>("app.info", "get_app_info")
    : invoke<AppInfo>("get_app_info");

export const saveConfig = (config: Config) =>
  isHubEmbedded()
    ? hubInvoke<StateDto>("config.save", "save_config", { config })
    : invoke<StateDto>("save_config", { config });

export const previewSync = (pairId?: string | null) =>
  isHubEmbedded()
    ? hubTaskRequest<Preview>("sync.preview", { pairId: pairId ?? null })
    : invoke<Preview>("preview_sync", { request: { pairId: pairId ?? null } });

export const startSync = (pairId?: string | null, trigger = "manual") =>
  isHubEmbedded()
    ? hubTaskRequest("sync.start", { pairId: pairId ?? null, trigger })
    : invoke<void>("start_sync", {
        request: { pairId: pairId ?? null, trigger },
      });

export const getHistory = (limit?: number) =>
  isHubEmbedded()
    ? hubInvoke<{ rows: LogRow[] }>("history.list", "get_history", {
        limit: limit ?? null,
      }).then((r) => r.rows)
    : invoke<LogRow[]>("get_history", { limit: limit ?? null });

export const clearHistory = () =>
  isHubEmbedded()
    ? hubInvoke("history.clear", "clear_history").then(() => undefined)
    : invoke<void>("clear_history");

export const cleanupVersions = (pairId?: string | null) =>
  isHubEmbedded()
    ? hubInvoke<{ count: number }>("versions.cleanup", "cleanup_versions", {
        pairId: pairId ?? null,
      }).then((r) => r.count)
    : invoke<number>("cleanup_versions", {
        request: { pairId: pairId ?? null },
      });

export const testFilter = (include: string[], exclude: string[], path: string) =>
  isHubEmbedded()
    ? hubInvoke<FilterTestDto>("filter.test", "test_filter", {
        include,
        exclude,
        path,
      })
    : invoke<FilterTestDto>("test_filter", { include, exclude, path });

export const exportHistory = (path: string, rows: LogRow[]) =>
  isHubEmbedded()
    ? hubInvoke<{ count: number }>("history.export", "export_history", {
        path,
        rows,
      }).then((r) => r.count)
    : invoke<number>("export_history", { path, rows });

// 暂停仅作用于 GUI 进程内的同步器；宿主模式暂不支持（无在线同步器）
export const setSyncPaused = (paused: boolean) =>
  isHubEmbedded()
    ? Promise.resolve()
    : invoke<void>("set_sync_paused", { paused });

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
