import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { scanCancel, scanPoll, scanReset, scanStart } from "../api";
import type { ScanReport, ScanTerminal } from "../types";

const TYPE_CHIPS = ["zip", "png", "jpg", "gif", "webp", "pdf"];

const SEVERITY_LABEL: Record<string, string> = {
  ok: "正常",
  warning: "可疑",
  corrupt: "损坏",
  unknown: "未知",
};

function worstOf(report: ScanReport): string {
  const rank = (s: string) =>
    s === "corrupt" ? 3 : s === "unknown" ? 2 : s === "warning" ? 1 : 0;
  return report.findings.reduce(
    (acc, f) => (rank(f.severity) > rank(acc) ? f.severity : acc),
    "ok",
  );
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

interface Props {
  pluginId: string;
}

export function PulsePanel({ pluginId }: Props) {
  const [path, setPath] = useState("");
  const [types, setTypes] = useState<string[]>([]);
  const [deep, setDeep] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(
    null,
  );
  const [terminal, setTerminal] = useState<ScanTerminal | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const scanningRef = useRef(false);
  scanningRef.current = scanning;

  useEffect(() => {
    if (!scanning) return;
    const timer = window.setInterval(async () => {
      try {
        const poll = await scanPoll(pluginId);
        if (poll.taskId) setTaskId(poll.taskId);
        if (poll.notifications.length > 0) {
          const last = poll.notifications[poll.notifications.length - 1];
          setProgress({ done: last.done, total: last.total });
        }
        if (poll.error) {
          setError(poll.error);
          setScanning(false);
        } else if (poll.terminal) {
          setTerminal(poll.terminal);
          setProgress(null);
          setScanning(false);
        }
      } catch (e) {
        setError(String(e));
        setScanning(false);
      }
    }, 400);
    return () => window.clearInterval(timer);
  }, [scanning, pluginId]);

  const pickDir = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (picked && !Array.isArray(picked)) setPath(picked);
  };

  const toggleType = (t: string) => {
    setTypes((prev) =>
      prev.includes(t) ? prev.filter((x) => x !== t) : [...prev, t],
    );
  };

  const start = async () => {
    setError(null);
    setTerminal(null);
    setProgress(null);
    setTaskId(null);
    setCancelling(false);
    try {
      await scanReset(pluginId);
      await scanStart(pluginId, path, types, deep);
      setScanning(true);
    } catch (e) {
      setError(String(e));
    }
  };

  const cancel = async () => {
    setCancelling(true);
    try {
      await scanCancel(pluginId);
    } catch (e) {
      setError(String(e));
      setCancelling(false);
    }
  };

  const retry = async () => {
    // scan_reset 清终态；scan_start 若发现 sidecar 已死会自动重拉
    await start();
  };

  const problemReports = terminal?.reports.filter((r) => worstOf(r) !== "ok") ?? [];
  const visible = showAll ? (terminal?.reports ?? []) : problemReports;

  return (
    <div className="card pulse-panel">
      <h2>损坏检测（sidecar 扫描）</h2>

      <div className="field">
        <label>扫描路径</label>
        <div className="row">
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="选择文件或目录…"
            spellCheck={false}
          />
          <button className="btn ghost" onClick={pickDir} disabled={scanning}>
            浏览…
          </button>
        </div>
      </div>

      <div className="field">
        <label>类型过滤（不选 = 全部）</label>
        <div className="chip-row">
          {TYPE_CHIPS.map((t) => (
            <button
              key={t}
              className={"chip" + (types.includes(t) ? " active" : "")}
              onClick={() => toggleType(t)}
              disabled={scanning}
            >
              {t}
            </button>
          ))}
          {types.length > 0 && (
            <button className="chip clear" onClick={() => setTypes([])} disabled={scanning}>
              全部
            </button>
          )}
          <label className="deep-toggle">
            <input
              type="checkbox"
              checked={deep}
              onChange={(e) => setDeep(e.target.checked)}
              disabled={scanning}
            />
            强制深度解码
          </label>
        </div>
      </div>

      <div className="row pulse-actions">
        {!scanning ? (
          <button className="btn primary" onClick={start} disabled={!path}>
            {terminal || error ? "重新扫描" : "开始扫描"}
          </button>
        ) : (
          <button className="btn danger" onClick={cancel} disabled={cancelling}>
            {cancelling ? "取消中…" : "取消"}
          </button>
        )}
        {error && !scanning && (
          <button className="btn" onClick={retry}>
            重试
          </button>
        )}
      </div>

      {scanning && (
        <div className="progress-wrap">
          <div
            className="progress-bar"
            style={{
              width: progress
                ? `${Math.round((progress.done / Math.max(progress.total, 1)) * 100)}%`
                : "0%",
            }}
          />
          <span className="progress-text">
            {progress
              ? `${progress.done} / ${progress.total}`
              : taskId
                ? `任务 ${taskId} 启动中…`
                : "等待任务确认…"}
          </span>
        </div>
      )}

      {error && (
        <div className="scan-error">
          <span>{error}</span>
          <span className="hint">（宿主未卡死；修复后点「重试」会自动重拉 sidecar）</span>
        </div>
      )}

      {terminal && (
        <div className="scan-summary">
          <span className={"badge " + (terminal.bad > 0 ? "warn" : "ok")}>
            正常 {terminal.ok} · 问题 {terminal.bad}
          </span>
          <span className="badge">
            可疑 {terminal.warning} · 损坏 {terminal.corrupt} · 未知 {terminal.unknown}
          </span>
          <span className="badge">
            扫描 {terminal.scanned} / 发现 {terminal.discovered}
          </span>
          <span className="badge">{(terminal.elapsedMs / 1000).toFixed(2)}s</span>
          {terminal.cancelled && <span className="badge warn">已取消（部分结果）</span>}
          <button className="chip" onClick={() => setShowAll((v) => !v)}>
            {showAll ? "只看问题文件" : `显示全部（${terminal.reports.length}）`}
          </button>
        </div>
      )}

      {terminal && visible.length > 0 && (
        <table className="plugin-table result-table">
          <thead>
            <tr>
              <th>结论</th>
              <th>文件</th>
              <th>类型</th>
              <th>问题</th>
            </tr>
          </thead>
          <tbody>
            {visible.map((r) => {
              const worst = worstOf(r);
              const problems = r.findings.filter((f) => f.severity !== "ok");
              return (
                <tr key={r.path}>
                  <td>
                    <span
                      className={
                        "badge " +
                        (worst === "ok" ? "ok" : worst === "corrupt" ? "warn" : "warn")
                      }
                    >
                      {SEVERITY_LABEL[worst] ?? worst}
                    </span>
                  </td>
                  <td title={r.path}>{fileName(r.path)}</td>
                  <td>{r.file_type}</td>
                  <td>
                    {problems.length === 0
                      ? "—"
                      : problems.map((f) => `${f.code}: ${f.message}`).join("；")}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      {terminal && terminal.bad === 0 && !terminal.cancelled && (
        <p className="hint">全部文件通过检测，未发现问题。</p>
      )}
    </div>
  );
}
