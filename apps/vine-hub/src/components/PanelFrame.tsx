import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  configGet,
  configSet,
  hostLog,
  resolvePanelUi,
  taskCancelId,
  taskList,
  taskRetry,
  taskStart,
} from "../api";
import type { PanelView } from "../types";

/** 桥接错误码（docs/bridge-spec.md §2） */
const BRIDGE_METHOD_NOT_ALLOWED = -32010;
const BRIDGE_INTERNAL = -32005;

const WHITELIST = new Set([
  "fs.pick",
  "config.get",
  "config.set",
  "task.start",
  "task.list",
  "task.cancel",
  "task.retry",
  "notify",
  "log",
]);

interface BridgeRequest {
  channel: string;
  v: number;
  id: number | string;
  method: string;
  params?: Record<string, unknown>;
}

interface Props {
  pluginId: string;
  panel: PanelView;
  onNotify: (level: "info" | "error", message: string) => void;
}

export function PanelFrame({ pluginId, panel, onNotify }: Props) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  // 面板切换时解析 UI 路径 → asset 协议 URL
  useEffect(() => {
    let alive = true;
    setLoading(true);
    setLoadError(null);
    setSrc(null);
    void (async () => {
      try {
        const path = await resolvePanelUi(pluginId, panel.id);
        if (!alive) return;
        if (!path) {
          setLoadError(`面板入口不存在：${panel.ui}（请先构建插件前端 dist）`);
          setLoading(false);
          return;
        }
        setSrc(convertFileSrc(path));
      } catch (e) {
        if (!alive) return;
        setLoadError(String(e));
        setLoading(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [pluginId, panel.id, panel.ui]);

  const dispatch = useCallback(
    async (method: string, params: Record<string, unknown>) => {
      switch (method) {
        case "fs.pick": {
          const filters = (params.filters as { name: string; extensions: string[] }[]) ?? undefined;
          const multiple = params.multiple === true;
          const picked = await open({ multiple, filters });
          const paths = !picked
            ? []
            : Array.isArray(picked)
              ? picked
              : [picked];
          return { paths };
        }
        case "config.get": {
          const key = String(params.key ?? "");
          if (!key) throw bridgeError(-32602, "缺少 key");
          return { value: await configGet(pluginId, key) };
        }
        case "config.set": {
          const key = String(params.key ?? "");
          if (!key) throw bridgeError(-32602, "缺少 key");
          await configSet(pluginId, key, String(params.value ?? ""));
          return { ok: true };
        }
        case "task.start": {
          const target = String(params.pluginId ?? pluginId);
          if (target !== pluginId) {
            throw bridgeError(BRIDGE_METHOD_NOT_ALLOWED, "task.start 的 pluginId 必须等于本面板插件");
          }
          const rpcMethod = String(params.method ?? "");
          if (!rpcMethod) throw bridgeError(-32602, "缺少 method");
          const rpcParams = (params.params as Record<string, unknown>) ?? {};
          const taskId = await taskStart(pluginId, rpcMethod, rpcParams);
          return { taskId };
        }
        case "task.list": {
          const filter = params.pluginId ? String(params.pluginId) : undefined;
          return { tasks: await taskList(filter) };
        }
        case "task.cancel": {
          await taskCancelId(String(params.taskId ?? ""));
          return { ok: true };
        }
        case "task.retry": {
          const taskId = await taskRetry(String(params.taskId ?? ""));
          return { taskId };
        }
        case "notify": {
          const level = params.level === "error" ? "error" : "info";
          onNotify(level, String(params.message ?? ""));
          return { ok: true };
        }
        case "log": {
          const level = String(params.level ?? "info");
          await hostLog(pluginId, level, String(params.message ?? ""));
          return { ok: true };
        }
        default:
          throw bridgeError(BRIDGE_METHOD_NOT_ALLOWED, `方法不在白名单: ${method}`);
      }
    },
    [pluginId, onNotify],
  );

  // 桥接消息循环
  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const data = event.data as BridgeRequest | null;
      if (!data || data.channel !== "vinehub" || data.v !== 1) return;
      if (event.source !== iframeRef.current?.contentWindow) return;
      if (data.id === undefined || data.id === null) return;

      void (async () => {
        try {
          if (!WHITELIST.has(data.method)) {
            throw bridgeError(BRIDGE_METHOD_NOT_ALLOWED, `方法不在白名单: ${data.method}`);
          }
          const result = await dispatch(data.method, data.params ?? {});
          iframeRef.current?.contentWindow?.postMessage(
            { channel: "vinehub", v: 1, id: data.id, result },
            "*",
          );
        } catch (err) {
          const code =
            typeof err === "object" && err !== null && "code" in err
              ? Number((err as { code: number }).code)
              : BRIDGE_INTERNAL;
          const message = String(
            typeof err === "object" && err !== null && "message" in err
              ? (err as { message: string }).message
              : err,
          );
          iframeRef.current?.contentWindow?.postMessage(
            { channel: "vinehub", v: 1, id: data.id, error: { code, message } },
            "*",
          );
        }
      })();
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [dispatch]);

  return (
    <div className="card panel-frame-card">
      <h2>{panel.title}</h2>
      {loadError && <div className="scan-error">{loadError}</div>}
      {loading && !loadError && <p className="hint">面板加载中…</p>}
      {src && (
        <iframe
          ref={iframeRef}
          className="panel-frame"
          src={src}
          title={`${pluginId}#${panel.id}`}
          sandbox="allow-scripts allow-forms allow-modals allow-popups allow-clipboard-read allow-clipboard-write"
          allow="clipboard-read; clipboard-write"
          onLoad={() => setLoading(false)}
        />
      )}
    </div>
  );
}

function bridgeError(code: number, message: string): Error & { code: number } {
  const e = new Error(message) as Error & { code: number };
  e.code = code;
  return e;
}
