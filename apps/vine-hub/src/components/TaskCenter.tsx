import { useCallback, useEffect, useRef, useState } from "react";
import { taskCancelId, taskList, taskRetry } from "../api";
import type { PluginView, TaskView } from "../types";

const METHOD_TITLES: Record<string, string> = {
  "scan.start": "扫描",
  "plan.start": "整理计划",
  "dupes.find": "查重",
  "export.start": "导出",
  "sync.preview": "同步预览",
  "sync.start": "执行同步",
  "library.addRoot": "添加资料库根",
  "library.roots": "资料库列表",
  "library.scan": "资料库扫描",
  "search.items": "搜索",
  "file.convert": "光标转换",
};

function methodTitle(method: string): string {
  return METHOD_TITLES[method] ?? method;
}

function statusBadge(status: string): { cls: string; label: string } {
  switch (status) {
    case "running":
      return { cls: "badge running", label: "进行中" };
    case "done":
      return { cls: "badge ok", label: "完成" };
    case "failed":
      return { cls: "badge danger", label: "失败" };
    case "cancelled":
      return { cls: "badge warn", label: "已取消" };
    default:
      return { cls: "badge", label: status };
  }
}

function timeLabel(ms: number): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function summaryOf(task: TaskView): string {
  if (task.status === "failed" || task.status === "cancelled" || task.status === "done") {
    if (!task.detail) return "—";
    try {
      const parsed = JSON.parse(task.detail) as { summary?: string };
      return parsed.summary ?? task.detail;
    } catch {
      return task.detail;
    }
  }
  return task.progress?.msg ?? "进行中";
}

interface Props {
  plugins: PluginView[];
}

export function TaskCenter({ plugins }: Props) {
  const [tasks, setTasks] = useState<TaskView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const aliveRef = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const list = await taskList();
      if (!aliveRef.current) return;
      setTasks(list);
      setError(null);
    } catch (e) {
      if (!aliveRef.current) return;
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    aliveRef.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1000);
    return () => {
      aliveRef.current = false;
      window.clearInterval(timer);
    };
  }, [refresh]);

  const pluginName = (id: string) => plugins.find((p) => p.id === id)?.name ?? id;

  const handleCancel = async (taskId: string) => {
    setBusy(true);
    try {
      await taskCancelId(taskId);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRetry = async (taskId: string) => {
    setBusy(true);
    try {
      await taskRetry(taskId);
      setError(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const running = tasks.filter((t) => t.status === "running").length;

  return (
    <section className="detail">
      <header className="detail-header">
        <div>
          <h1>任务中心</h1>
          <div className="detail-subtitle">
            跨插件任务列表 · 进行中 {running} / 共 {tasks.length}
          </div>
        </div>
        <div className="detail-actions">
          <button className="btn" onClick={() => void refresh()}>
            刷新
          </button>
        </div>
      </header>

      {error && <div className="banner error"><span>{error}</span></div>}

      <div className="card">
        <p className="hint">
          所有 sidecar 插件的任务在此汇总；进度来自 <code>task.progress</code> 通知，
          可随时取消或对失败任务重试（使用发起时的参数快照）。
        </p>
        {tasks.length === 0 ? (
          <div className="empty-state">暂无任务记录。在各插件面板发起操作后会出现在这里。</div>
        ) : (
          <table className="plugin-table task-table">
            <thead>
              <tr>
                <th>任务</th>
                <th>插件</th>
                <th>状态</th>
                <th>进度 / 摘要</th>
                <th>发起时间</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {tasks.map((t) => {
                const badge = statusBadge(t.status);
                const pct =
                  t.progress && t.progress.total > 0
                    ? Math.min(100, Math.round((t.progress.done / t.progress.total) * 100))
                    : null;
                return (
                  <tr key={t.id}>
                    <td>
                      <div>{methodTitle(t.method)}</div>
                      <code className="task-id">{t.id}</code>
                    </td>
                    <td>{pluginName(t.pluginId)}</td>
                    <td>
                      <span className={badge.cls}>{badge.label}</span>
                    </td>
                    <td className="task-summary">
                      {t.status === "running" && pct !== null ? (
                        <div className="progress-wrap">
                          <div className="progress-bar" style={{ width: `${pct}%` }} />
                          <span className="progress-text">
                            {t.progress!.done}/{t.progress!.total}（{pct}%）
                          </span>
                        </div>
                      ) : (
                        <span className="task-detail">{summaryOf(t)}</span>
                      )}
                    </td>
                    <td>{timeLabel(t.createdAt)}</td>
                    <td>
                      <div className="task-actions">
                        {t.status === "running" && (
                          <button
                            className="btn danger small"
                            disabled={busy}
                            onClick={() => void handleCancel(t.id)}
                          >
                            取消
                          </button>
                        )}
                        {t.status !== "running" && (
                          <button
                            className="btn small"
                            disabled={busy}
                            onClick={() => void handleRetry(t.id)}
                          >
                            重试
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}
