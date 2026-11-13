import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { isHubEmbedded } from "./host-bridge";
import { confirm } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import type {
  Config,
  FinishedEvt,
  LogRow,
  PairConfig,
  Preview,
  ProgressEvt,
  ScanProgressEvt,
  StartedEvt,
  StateDto,
} from "./types";
import { I18nProvider, useI18n } from "./i18n";
import { formatBytes } from "./utils";
import PairCard from "./components/PairCard";
import PairEditor from "./components/PairEditor";
import PreviewModal from "./components/PreviewModal";
import SettingsView from "./components/SettingsView";
import HistoryView from "./components/HistoryView";
import {
  IconHistory,
  IconHome,
  IconMoon,
  IconPlus,
  IconSettings,
  IconSun,
  IconSync,
} from "./components/Icons";

export default function App() {
  return (
    <I18nProvider>
      <AppShell />
    </I18nProvider>
  );
}

type Tab = "home" | "history" | "settings";

interface Toast {
  id: number;
  kind: "ok" | "err" | "warn";
  title: string;
  desc?: string;
}

function AppShell() {
  const { t, setBase, setPreview: setLangPreview } = useI18n();
  const [state, setState] = useState<StateDto | null>(null);
  const [tab, setTab] = useState<Tab>("home");
  const [history, setHistory] = useState<LogRow[]>([]);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [progress, setProgress] = useState<ProgressEvt | null>(null);
  const [scan, setScan] = useState<ScanProgressEvt | null>(null);
  const [rate, setRate] = useState({ bytesPerSec: 0, filesPerSec: 0 });
  const [scanRate, setScanRate] = useState(0);
  const [paused, setPaused] = useState(false);
  const rateRef = useRef({ lastT: 0, lastBytes: 0, lastDone: 0, bytesPerSec: 0, filesPerSec: 0 });
  const scanRateRef = useRef({ lastT: 0, lastFiles: 0, filesPerSec: 0 });

  const fmtDur = (sec: number) => {
    const s = Math.max(1, Math.round(sec));
    if (s < 60) return t("durSec", { n: s });
    if (s < 3600) return t("durMin", { m: Math.floor(s / 60), s: s % 60 });
    return t("durHour", { h: Math.floor(s / 3600), m: Math.floor((s % 3600) / 60) });
  };

  const togglePause = async () => {
    const next = !paused;
    setPaused(next);
    if (next) setRate({ bytesPerSec: 0, filesPerSec: 0 });
    try {
      await api.setSyncPaused(next);
    } catch {
      setPaused(!next);
    }
  };
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewPairId, setPreviewPairId] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [starting, setStarting] = useState(false);
  const [editing, setEditing] = useState<{
    mode: "new" | "edit";
    pair: PairConfig;
  } | null>(null);
  const [effectiveDark, setEffectiveDark] = useState(true);

  const toast = useCallback(
    (kind: Toast["kind"], title: string, desc?: string) => {
      const id = Date.now() + Math.random();
      setToasts((x) => [...x, { id, kind, title, desc }]);
      window.setTimeout(
        () => setToasts((x) => x.filter((y) => y.id !== id)),
        kind === "err" ? 9000 : 5500,
      );
    },
    [],
  );

  const refresh = useCallback(async () => {
    try {
      setState(await api.getState());
    } catch (e) {
      toast("err", t("errLoadState"), String(e));
    }
  }, [toast, t]);

  const loadHistory = useCallback(async () => {
    try {
      setHistory(await api.getHistory(300));
    } catch {
      /* 忽略历史加载错误 */
    }
  }, []);

  useEffect(() => {
    void refresh();
    void loadHistory();
  }, [refresh, loadHistory]);

  // 配置语言作为基准语言（初始加载与保存后同步）
  useEffect(() => {
    if (state) setBase(state.config.language);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state?.config.language]);

  // 后端事件（宿主 iframe 无 Tauri 事件系统则 no-op）
  useEffect(() => {
    function safeListen<T>(
      event: string,
      cb: (e: { payload: T }) => void,
    ): Promise<() => void> {
      if (isHubEmbedded()) return Promise.resolve(() => {});
      return listen<T>(event, cb);
    }
    const unsubs: Array<Promise<() => void>> = [
      safeListen<StartedEvt>("sync-started", () => {
        setProgress(null);
        setPaused(false);
        rateRef.current = { lastT: 0, lastBytes: 0, lastDone: 0, bytesPerSec: 0, filesPerSec: 0 };
        scanRateRef.current = { lastT: 0, lastFiles: 0, filesPerSec: 0 };
        setRate({ bytesPerSec: 0, filesPerSec: 0 });
        setScanRate(0);
        void refresh();
      }),
      safeListen<ProgressEvt>("sync-progress", (e) => {
        const p = e.payload;
        const st = rateRef.current;
        const now = performance.now();
        const dt = st.lastT ? (now - st.lastT) / 1000 : 0;
        if (st.lastT && dt > 0.05 && dt < 5 && p.done >= st.lastDone && p.bytes >= st.lastBytes) {
          const instB = (p.bytes - st.lastBytes) / dt;
          const instF = (p.done - st.lastDone) / dt;
          st.bytesPerSec = st.bytesPerSec > 0 ? st.bytesPerSec * 0.6 + instB * 0.4 : instB;
          st.filesPerSec = st.filesPerSec > 0 ? st.filesPerSec * 0.6 + instF * 0.4 : instF;
        }
        st.lastT = now;
        st.lastBytes = p.bytes;
        st.lastDone = p.done;
        setRate({ bytesPerSec: st.bytesPerSec, filesPerSec: st.filesPerSec });
        setProgress(p);
      }),
      safeListen<ScanProgressEvt>("scan-progress", (e) => {
        const p = e.payload;
        const st = scanRateRef.current;
        const now = performance.now();
        const dt = st.lastT ? (now - st.lastT) / 1000 : 0;
        if (st.lastT && dt > 0.05 && dt < 5 && p.files >= st.lastFiles) {
          const inst = (p.files - st.lastFiles) / dt;
          st.filesPerSec = st.filesPerSec > 0 ? st.filesPerSec * 0.6 + inst * 0.4 : inst;
        }
        st.lastT = now;
        st.lastFiles = p.files;
        setScanRate(st.filesPerSec);
        setScan(p);
      }),
      safeListen<FinishedEvt>("sync-finished", (e) => {
        const p = e.payload;
        setProgress(null);
        setScan(null);
        setPaused(false);
        void refresh();
        void loadHistory();
        const changes =
          p.counts.added + p.counts.updated + p.counts.deleted + p.counts.versioned;
        if (p.targetUnavailable) {
          toast("warn", t("toastTargetUnavailable"), p.message);
        } else if (!p.ok) {
          toast("err", t("toastSyncIssue"), p.message);
        } else if (changes > 0) {
          toast("ok", t("toastSyncDone"), p.message);
        } else if (p.trigger === "manual") {
          toast("ok", t("toastSyncDone"), t("toastNoChanges"));
        }
      }),
    ];
    return () => {
      unsubs.forEach((u) => void u.then((f) => f()));
    };
  }, [refresh, loadHistory, toast, t]);

  // 主题
  useEffect(() => {
    const theme = state?.config.theme ?? "system";
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const dark = theme === "dark" || (theme === "system" && mq.matches);
      document.documentElement.dataset.theme = dark ? "dark" : "light";
      setEffectiveDark(dark);
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [state?.config.theme]);

  // ---------- 动作 ----------

  const openPreview = async (pairId: string | null) => {
    setPreviewPairId(pairId);
    setPreviewing(true);
    setScan(null);
    try {
      const p = await api.previewSync(pairId);
      if (
        p.changes.length === 0 &&
        p.errors.length === 0 &&
        !p.targetUnavailable
      ) {
        await startSync(pairId);
        return;
      }
      setPreview(p);
    } catch (e) {
      toast("err", t("errPreview"), String(e));
    } finally {
      setPreviewing(false);
      setScan(null);
    }
  };

  const startSync = async (pairId: string | null) => {
    setStarting(true);
    try {
      await api.startSync(pairId);
    } catch (e) {
      toast("err", t("errStartSync"), String(e));
    } finally {
      setStarting(false);
    }
  };

  const handleSync = async (pairId: string | null) => {
    if (state?.syncing || starting) return;
    if (state?.config.confirmBeforeSync) {
      await openPreview(pairId);
    } else {
      await startSync(pairId);
    }
  };

  const confirmPreview = async () => {
    const pid = previewPairId;
    setPreview(null);
    await startSync(pid);
  };

  const applyConfig = async (config: Config): Promise<boolean> => {
    try {
      const s = await api.saveConfig(config);
      setState(s);
      // 保存成功：语言从预览切换为已保存的基准语言（避免闪回旧语言）
      setBase(s.config.language);
      setLangPreview(null);
      toast("ok", t("toastSaved"));
      return true;
    } catch (e) {
      toast("err", t("toastSaveFailed"), String(e));
      return false;
    }
  };

  const togglePair = async (pair: PairConfig, enabled: boolean) => {
    const next: Config = {
      ...state!.config,
      pairs: state!.config.pairs.map((p) =>
        p.id === pair.id ? { ...p, enabled } : p,
      ),
    };
    await applyConfig(next);
  };

  const removePair = async (pair: PairConfig) => {
    const ok = await confirm(t("confirmDeletePair", { name: pair.name }), {
      title: t("confirmDeleteTitle"),
    });
    if (!ok) return;
    const next: Config = {
      ...state!.config,
      pairs: state!.config.pairs.filter((p) => p.id !== pair.id),
    };
    await applyConfig(next);
    void loadHistory();
  };

  const toggleThemeQuick = () => {
    if (!state) return;
    void applyConfig({
      ...state.config,
      theme: effectiveDark ? "light" : "dark",
    });
  };

  // ---------- 渲染 ----------

  const syncing = state?.syncing ?? false;
  const queued = state?.queued ?? false;
  const pairs = state?.config.pairs ?? [];
  const stateById = new Map((state?.pairStates ?? []).map((s) => [s.pairId, s]));

  const statusLabel = syncing
    ? queued
      ? t("statusSyncingQueued")
      : t("statusSyncing")
    : t("statusIdle");

  const showScan = Boolean(scan) && (syncing || previewing);

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <img src="/icon.png" alt="" />
          vineSync
          <span className="sub">{t("brandSub")}</span>
        </div>

        <nav className="nav">
          <button
            className={`nav-btn ${tab === "home" ? "active" : ""}`}
            onClick={() => setTab("home")}
          >
            <IconHome width={15} height={15} /> {t("navHome")}
          </button>
          <button
            className={`nav-btn ${tab === "history" ? "active" : ""}`}
            onClick={() => setTab("history")}
          >
            <IconHistory width={15} height={15} /> {t("navHistory")}
          </button>
          <button
            className={`nav-btn ${tab === "settings" ? "active" : ""}`}
            onClick={() => setTab("settings")}
          >
            <IconSettings width={15} height={15} /> {t("navSettings")}
          </button>
        </nav>

        <div className="topbar-right">
          <div className="status-pill">
            <span
              className={`status-dot ${syncing ? "busy" : pairs.length > 0 ? "ok" : ""}`}
            />
            {statusLabel}
          </div>
          <button
            className="icon-btn"
            onClick={toggleThemeQuick}
            title={t("themeToggleTitle")}
          >
            {effectiveDark ? (
              <IconSun width={16} height={16} />
            ) : (
              <IconMoon width={16} height={16} />
            )}
          </button>
        </div>
      </header>

      <main className="content">
        <div className="content-inner" key={tab}>
          {tab === "home" && (
            <>
              <div className="toolbar">
                <h2>{t("folderPairs")}</h2>
                <span className="hint">
                  {pairs.length > 0
                    ? t("enabledCount", {
                        on: pairs.filter((p) => p.enabled).length,
                        total: pairs.length,
                      })
                    : t("emptyHint")}
                </span>
                <div className="spacer" />
                <button
                  className="btn btn-primary btn-lg"
                  disabled={syncing || starting || previewing || pairs.length === 0}
                  onClick={() => void handleSync(null)}
                >
                  <IconSync
                    width={17}
                    height={17}
                    className={syncing ? "spin" : ""}
                  />
                  {syncing ? t("btnSyncing") : t("btnSyncNow")}
                </button>
                <button
                  className="btn"
                  onClick={() =>
                    setEditing({
                      mode: "new",
                      pair: {
                        id: api.newPairId(),
                        name: "",
                        source: "",
                        dest: "",
                        enabled: true,
                        include: [],
                        exclude: [],
                        intervalMinutes: null,
                      },
                    })
                  }
                >
                  <IconPlus width={16} height={16} /> {t("btnNewPair")}
                </button>
              </div>

              {showScan && scan && (
                <div className="progress-wrap">
                  <strong style={{ fontSize: 13 }}>
                    {t("scanScanning")} · {scan.pairName}
                  </strong>
                  <span className="progress-text">
                    {scan.phase === "source"
                      ? t("scanSource")
                      : scan.phase === "target"
                        ? t("scanTarget")
                        : t("scanHash")}{" "}
                    ·{" "}
                    {scan.total > 0
                      ? `${scan.files} / ${scan.total}`
                      : t("scanFmt", { n: scan.files })}
                    {scan.phase !== "hash" && scanRate > 0 && <> · {Math.round(scanRate)}/s</>}
                  </span>
                  <span className="progress-file">{scan.relPath}</span>
                </div>
              )}

              {syncing && progress && progress.total > 0 && !showScan && (
                <div className="progress-wrap">
                  <strong style={{ fontSize: 13 }}>
                    {progress.pairName}
                  </strong>
                  <div className="progress-bar">
                    <div
                      className="progress-fill"
                      style={{
                        width: `${Math.round((progress.done / progress.total) * 100)}%`,
                      }}
                    />
                  </div>
                  <span className="progress-file">{progress.relPath}</span>
                  <span className="progress-text">
                    {progress.done} / {progress.total}
                    {rate.bytesPerSec > 0 && <> · {formatBytes(rate.bytesPerSec)}/s</>}
                    {rate.filesPerSec > 0 && progress.done < progress.total && !paused && (
                      <>
                        {" · "}
                        {t("etaT", {
                          t: fmtDur((progress.total - progress.done) / rate.filesPerSec),
                        })}
                      </>
                    )}
                  </span>
                  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    <button className="btn btn-sm" onClick={() => void togglePause()}>
                      {paused ? t("btnResume") : t("btnPause")}
                    </button>
                    {paused && <span className="chip warn">{t("pausedChip")}</span>}
                  </div>
                </div>
              )}

              {pairs.length === 0 ? (
                <div className="empty">
                  <div className="big">{t("emptyNoPairs")}</div>
                  <p>
                    {t("emptyNoPairsDesc1")}
                    <br />
                    {t("emptyNoPairsDesc2")}
                  </p>
                  <button
                    className="btn btn-primary"
                    onClick={() =>
                      setEditing({
                        mode: "new",
                        pair: {
                          id: api.newPairId(),
                          name: "",
                          source: "",
                          dest: "",
                          enabled: true,
                          include: [],
                          exclude: [],
                          intervalMinutes: null,
                        },
                      })
                    }
                  >
                    <IconPlus width={16} height={16} /> {t("btnNewPair")}
                  </button>
                </div>
              ) : (
                <div className="grid">
                  {pairs.map((p) => (
                    <PairCard
                      key={p.id}
                      pair={p}
                      state={stateById.get(p.id)}
                      syncing={syncing}
                      onSync={() => void handleSync(p.id)}
                      onPreview={() => void openPreview(p.id)}
                      onEdit={() => setEditing({ mode: "edit", pair: p })}
                      onToggle={(v) => void togglePair(p, v)}
                      onDelete={() => void removePair(p)}
                    />
                  ))}
                </div>
              )}
            </>
          )}

          {tab === "history" && (
            <HistoryView
              rows={history}
              onRefresh={() => void loadHistory()}
              onClear={async () => {
                const ok = await confirm(t("confirmClearHistory"), {
                  title: t("confirmClearTitle"),
                });
                if (!ok) return;
                try {
                  await api.clearHistory();
                  await loadHistory();
                  toast("ok", t("toastHistoryCleared"));
                } catch (e) {
                  toast("err", t("toastClearFailed"), String(e));
                }
              }}
            />
          )}

          {tab === "settings" && state && (
            <SettingsView
              config={state.config}
              onSave={applyConfig}
              onToast={(kind, title, desc) => toast(kind, title, desc)}
            />
          )}
        </div>
      </main>

      {editing && state && (
        <PairEditor
          mode={editing.mode}
          pair={editing.pair}
          config={state.config}
          onClose={() => setEditing(null)}
          onSave={async (config) => {
            const ok = await applyConfig(config);
            if (ok) setEditing(null);
            return ok;
          }}
        />
      )}

      {preview && (
        <PreviewModal
          preview={preview}
          syncing={syncing}
          onCancel={() => setPreview(null)}
          onConfirm={() => void confirmPreview()}
        />
      )}

      <div className="toasts">
        {toasts.map((x) => (
          <div key={x.id} className={`toast ${x.kind}`}>
            <div style={{ flex: 1 }}>
              <div className="t">{x.title}</div>
              {x.desc && <div className="d">{x.desc}</div>}
            </div>
            <button
              className="close"
              onClick={() => setToasts((y) => y.filter((i) => i.id !== x.id))}
            >
              ×
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
