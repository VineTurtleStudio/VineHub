/**
 * VineHub 双模适配层（docs/bridge-spec.md §4）。
 *
 * - 宿主模式（iframe 内，URL 带 `vinehub=1`）：postMessage 桥接，
 *   业务 RPC 经宿主代理到本插件 sidecar（`rpc.request`）。
 * - 独立运行：原生 `invoke`，行为与改造前完全一致。
 *
 * 所有 Tauri 调用收敛在 `src/api.ts`；本模块只提供通道，不感知业务。
 */

export interface BridgeErrorBody {
  code: number;
  message: string;
}

interface PendingEntry {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
  timer: number;
}

const pending = new Map<number, PendingEntry>();
let seq = 0;
const BRIDGE_TIMEOUT_MS = 30 * 60 * 1000;

/** 是否运行在 VineHub 宿主 iframe 内。 */
export function isHubEmbedded(): boolean {
  try {
    return (
      window.parent !== window &&
      new URLSearchParams(window.location.search).get("vinehub") === "1"
    );
  } catch {
    return false;
  }
}

/** 宿主桥接原始请求（仅 hub 模式可用）。 */
export function bridgeRequest<T = unknown>(
  method: string,
  params?: Record<string, unknown>,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (!isHubEmbedded()) {
      reject(new Error("bridgeRequest 仅可在宿主 iframe 内使用"));
      return;
    }
    const id = ++seq;
    const timer = window.setTimeout(() => {
      pending.delete(id);
      reject(new Error(`桥接超时: ${method}`));
    }, BRIDGE_TIMEOUT_MS);
    pending.set(id, {
      resolve: resolve as (v: unknown) => void,
      reject,
      timer,
    });
    window.parent.postMessage(
      { channel: "vinehub", v: 1, id, method, params: params ?? {} },
      "*",
    );
  });
}

// 宿主响应监听（模块加载时安装一次）
window.addEventListener("message", (event: MessageEvent) => {
  const data = event.data as {
    channel?: string;
    v?: number;
    id?: number | string;
    result?: unknown;
    error?: BridgeErrorBody;
  } | null;
  if (!data || data.channel !== "vinehub" || data.v !== 1) return;
  if (data.id === undefined || data.id === null) return;
  const entry = pending.get(data.id);
  if (!entry) return;
  pending.delete(data.id);
  window.clearTimeout(entry.timer);
  if (data.error) {
    const err = new Error(data.error.message) as Error & { code: number };
    err.code = data.error.code;
    entry.reject(err);
  } else {
    entry.resolve(data.result);
  }
});

/**
 * 双模调用入口：
 * - 独立运行 → `invoke(invokeName, args)`；
 * - 宿主模式 → `rpc.request { method: rpcMethod, params: args }`。
 *
 * `rpcMethod` 为 null 时该命令仅独立可用（宿主模式直接拒绝）。
 */
export async function hubInvoke<T = unknown>(
  rpcMethod: string | null,
  invokeName: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (!isHubEmbedded()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(invokeName, args);
  }
  if (!rpcMethod) {
    throw new Error(`命令 ${invokeName} 不可在宿主模式下调用`);
  }
  return bridgeRequest<T>("rpc.request", { method: rpcMethod, params: args });
}

/** 宿主通知（notify 横幅）；独立模式 no-op。 */
export async function hostNotify(
  level: "info" | "error",
  message: string,
): Promise<void> {
  if (!isHubEmbedded()) return;
  await bridgeRequest("notify", { level, message });
}

/** 宿主日志（写 {logs}/{pluginId}-ui.log）；独立模式 no-op。 */
export async function hostLogLine(
  level: "debug" | "info" | "warn" | "error",
  message: string,
): Promise<void> {
  if (!isHubEmbedded()) return;
  await bridgeRequest("log", { level, message });
}
