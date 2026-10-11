import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getPluginsDir,
  importPluginZip,
  launchPlugin,
  listPlugins,
  rescanPlugins,
  setEntryOverride,
  setPluginEnabled,
} from "./api";
import type { PluginView } from "./types";
import { PluginDetail } from "./components/PluginDetail";
import { SettingsPanel } from "./components/SettingsPanel";
import { TaskCenter } from "./components/TaskCenter";

type Page =
  | { kind: "plugin"; id: string }
  | { kind: "settings" }
  | { kind: "tasks" };

export default function App() {
  const [plugins, setPlugins] = useState<PluginView[]>([]);
  const [page, setPage] = useState<Page>({ kind: "settings" });
  const [pluginsDir, setPluginsDir] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await listPlugins();
      setPlugins(list);
      setError(null);
      setPage((prev) => {
        if (prev.kind === "plugin" && !list.some((p) => p.id === prev.id)) {
          return list.length > 0 ? { kind: "plugin", id: list[0].id } : { kind: "settings" };
        }
        return prev;
      });
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void (async () => {
      await refresh();
      try {
        setPluginsDir(await getPluginsDir());
      } catch {
        /* ignore */
      }
    })();
  }, [refresh]);

  const selected =
    page.kind === "plugin" ? plugins.find((p) => p.id === page.id) : undefined;

  const handleLaunch = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await launchPlugin(id);
      setNotice("已启动");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleOverrideSave = async (id: string, path: string | null) => {
    setBusy(true);
    try {
      setPlugins(await setEntryOverride(id, path));
      setNotice(path ? "已保存路径覆盖" : "已清除路径覆盖");
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleToggleEnabled = async (plugin: PluginView) => {
    setBusy(true);
    try {
      setPlugins(await setPluginEnabled(plugin.id, !plugin.enabled));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRescan = async () => {
    setBusy(true);
    try {
      setPlugins(await rescanPlugins());
      setNotice("已重新扫描插件目录");
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleImport = async () => {
    const zipPath = await open({
      multiple: false,
      filters: [{ name: "VineHub 插件包", extensions: ["vpkg", "zip"] }],
    });
    if (!zipPath || Array.isArray(zipPath)) return;
    setBusy(true);
    try {
      const result = await importPluginZip(zipPath);
      setNotice(result.message);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark"> Vine</span>
          <span className="brand-name">VineHub</span>
        </div>

        <nav className="plugin-nav">
          <div className="nav-section-title">插件</div>
          {plugins.map((p) => (
            <button
              key={p.id}
              className={
                "plugin-item" +
                (page.kind === "plugin" && page.id === p.id ? " active" : "")
              }
              onClick={() => setPage({ kind: "plugin", id: p.id })}
            >
              <span
                className={
                  "status-dot " + (p.available ? "ok" : p.enabled ? "warn" : "off")
                }
              />
              <span className="plugin-item-name">{p.name}</span>
              <span className="plugin-item-version">v{p.version}</span>
            </button>
          ))}
          {plugins.length === 0 && (
            <div className="nav-empty">未发现插件</div>
          )}
        </nav>

        <div className="sidebar-footer">
          <button className="btn ghost" onClick={handleImport} disabled={busy}>
            导入插件包…
          </button>
          <button
            className={"btn ghost" + (page.kind === "tasks" ? " active" : "")}
            onClick={() => setPage({ kind: "tasks" })}
          >
            任务中心
          </button>
          <button
            className={"btn ghost" + (page.kind === "settings" ? " active" : "")}
            onClick={() => setPage({ kind: "settings" })}
          >
            设置
          </button>
        </div>
      </aside>

      <main className="main">
        {(notice || error) && (
          <div className={"banner " + (error ? "error" : "info")}>
            <span>{error ?? notice}</span>
            <button
              className="banner-close"
              onClick={() => {
                setNotice(null);
                setError(null);
              }}
            >
              ×
            </button>
          </div>
        )}

        {page.kind === "settings" && (
          <SettingsPanel
            pluginsDir={pluginsDir}
            plugins={plugins}
            onRescan={handleRescan}
            onSelectPlugin={(id) => setPage({ kind: "plugin", id })}
          />
        )}

        {page.kind === "tasks" && <TaskCenter plugins={plugins} />}

        {selected && (
          <PluginDetail
            plugin={selected}
            busy={busy}
            onLaunch={() => handleLaunch(selected.id)}
            onOverrideSave={(path) => handleOverrideSave(selected.id, path)}
            onToggleEnabled={() => handleToggleEnabled(selected)}
          />
        )}

        {page.kind === "plugin" && !selected && (
          <div className="empty-state">该插件已不在列表中，请重新扫描。</div>
        )}
      </main>
    </div>
  );
}
