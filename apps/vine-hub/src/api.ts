import { invoke } from "@tauri-apps/api/core";
import type { ImportResult, PluginView } from "./types";

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
