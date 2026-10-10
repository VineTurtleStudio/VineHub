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
