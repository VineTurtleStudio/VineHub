import { invoke } from "@tauri-apps/api/core";
import type {
  ImportResult,
  PluginView,
  ScanPoll,
  ScanStartResult,
  TaskView,
} from "./types";

export function listPlugins(): Promise<PluginView[]> {
  return invoke<PluginView[]>("list_plugins");
}

export function rescanPlugins(): Promise<PluginView[]> {
  return invoke<PluginView[]>("rescan_plugins");
}

export function launchPlugin(id: string): Promise<void> {
  return invoke<void>("launch_plugin", { id });
}

export function setEntryOverride(
  id: string,
  path: string | null,
): Promise<PluginView[]> {
  return invoke<PluginView[]>("set_entry_override", { id, path });
}

export function setPluginEnabled(
  id: string,
  enabled: boolean,
): Promise<PluginView[]> {
  return invoke<PluginView[]>("set_plugin_enabled", { id, enabled });
}

export function importPluginZip(zipPath: string): Promise<ImportResult> {
  return invoke<ImportResult>("import_plugin_zip", { zipPath });
}

export function getPluginsDir(): Promise<string> {
  return invoke<string>("get_plugins_dir");
}

export function scanStart(
  pluginId: string,
  path: string,
  types: string[],
  deep: boolean,
): Promise<ScanStartResult> {
  return invoke<ScanStartResult>("scan_start", {
    pluginId,
    path,
    types,
    deep,
  });
}

export function scanPoll(pluginId: string): Promise<ScanPoll> {
  return invoke<ScanPoll>("scan_poll", { pluginId });
}

export function scanCancel(pluginId: string): Promise<void> {
  return invoke<void>("scan_cancel", { pluginId });
}

export function scanReset(pluginId: string): Promise<void> {
  return invoke<void>("scan_reset", { pluginId });
}

export function sidecarStatus(pluginId: string): Promise<Record<string, unknown>> {
  return invoke("sidecar_status", { pluginId });
}

export function taskList(pluginId?: string): Promise<TaskView[]> {
  return invoke<TaskView[]>("task_list", { pluginId: pluginId ?? null });
}

export function taskCancelId(taskId: string): Promise<void> {
  return invoke<void>("task_cancel_id", { taskId });
}

export function taskRetry(taskId: string): Promise<string> {
  return invoke<string>("task_retry", { taskId });
}

export function taskStart(
  pluginId: string,
  method: string,
  params: Record<string, unknown>,
): Promise<string> {
  return invoke<string>("task_start", { pluginId, method, params });
}

export function resolvePanelUi(
  pluginId: string,
  panelId: string,
): Promise<string | null> {
  return invoke<string | null>("resolve_panel_ui", { pluginId, panelId });
}

export function configGet(pluginId: string, key: string): Promise<string | null> {
  return invoke<string | null>("config_get", { pluginId, key });
}

export function configSet(pluginId: string, key: string, value: string): Promise<void> {
  return invoke<void>("config_set", { pluginId, key, value });
}

export function hostLog(pluginId: string, level: string, message: string): Promise<void> {
  return invoke<void>("host_log", { pluginId, level, message });
}

export function rpcProxy(
  pluginId: string,
  method: string,
  params: Record<string, unknown> | null,
): Promise<unknown> {
  return invoke<unknown>("rpc_proxy", { pluginId, method, params });
}

export function rpcNotify(
  pluginId: string,
  method: string,
  params: Record<string, unknown> | null,
): Promise<void> {
  return invoke<void>("rpc_notify", { pluginId, method, params });
}
