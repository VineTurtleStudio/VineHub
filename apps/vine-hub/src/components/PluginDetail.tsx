import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { PluginView } from "../types";
import { PulsePanel } from "./PulsePanel";

interface Props {
  plugin: PluginView;
  busy: boolean;
  onLaunch: () => void;
  onOverrideSave: (path: string | null) => void;
  onToggleEnabled: () => void;
}

export function PluginDetail({ plugin, busy, onLaunch, onOverrideSave, onToggleEnabled }: Props) {
  const [draft, setDraft] = useState<string>(plugin.entryOverride ?? "");

  // 切换插件时同步草稿
  const [lastId, setLastId] = useState(plugin.id);
  if (lastId !== plugin.id) {
    setLastId(plugin.id);
    setDraft(plugin.entryOverride ?? "");
  }

  const pickExe = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "可执行文件", extensions: ["exe"] }],
    });
    if (path && !Array.isArray(path)) {
      setDraft(path);
    }
  };

  return (
    <section className="detail">
      <header className="detail-header">
        <div>
          <h1>{plugin.name}</h1>
          <div className="detail-subtitle">{plugin.id}</div>
        </div>
        <div className="detail-actions">
          <button
            className="btn primary"
            disabled={busy || !plugin.available}
            onClick={onLaunch}
            title={plugin.available ? "启动独立程序" : "不可启动：请检查入口路径"}
          >
            启动独立程序
          </button>
          <button className="btn ghost" onClick={onToggleEnabled} disabled={busy}>
            {plugin.enabled ? "停用" : "启用"}
          </button>
        </div>
      </header>

      <div className="badges">
        <span className="badge">{plugin.kind}</span>
        <span className="badge">v{plugin.version}</span>
        <span className="badge">api {plugin.apiVersion}</span>
        <span className={"badge " + (plugin.available ? "ok" : "warn")}>
          {plugin.available ? "可启动" : (plugin.statusReason ?? "不可用")}
        </span>
      </div>

      <div className="card">
        <h2>可执行文件</h2>
        <div className="field">
          <label>解析结果</label>
          <code className="path">{plugin.resolvedEntry ?? "（未找到）"}</code>
        </div>
        <div className="field">
          <label>路径覆盖（设置页同款能力）</label>
          <div className="row">
            <input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder="留空则按 manifest 的 entry 解析"
              spellCheck={false}
            />
            <button className="btn ghost" onClick={pickExe} disabled={busy}>
              浏览…
            </button>
            <button
              className="btn"
              disabled={busy || draft === (plugin.entryOverride ?? "")}
              onClick={() => onOverrideSave(draft.trim() === "" ? null : draft)}
            >
              保存
            </button>
            {plugin.entryOverride && (
              <button
                className="btn ghost"
                disabled={busy}
                onClick={() => {
                  setDraft("");
                  onOverrideSave(null);
                }}
              >
                清除
              </button>
            )}
          </div>
        </div>
      </div>

      {plugin.kind === "sidecar" && <PulsePanel pluginId={plugin.id} />}

      <div className="card">
        <h2>生命周期</h2>
        <p className="hint">
          {plugin.kind === "sidecar" ? (
            <>
              当前为 <code>sidecar</code> 形态：宿主拉起子进程走 JSON-RPC over
              stdio；独立 CLI 与 hub 插件共用同一 core，独立运行能力 100% 保留。
            </>
          ) : (
            <>
              当前为 <code>external</code> 形态：hub 仅负责一键启动独立程序，
              独立运行能力 100% 保留。升级 sidecar 只改 manifest 的 kind 字段。
            </>
          )}
        </p>
      </div>
    </section>
  );
}
