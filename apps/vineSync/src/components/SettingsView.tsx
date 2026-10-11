import { useEffect, useState } from "react";
import * as api from "../api";
import type { AppInfo, Config, DeletePolicy, Language, Theme, TriggerMode } from "../types";
import { useI18n, type Key } from "../i18n";
import { IconAlert, IconCheck, IconSync } from "./Icons";

interface Props {
  config: Config;
  onSave: (config: Config) => Promise<boolean>;
  onToast: (kind: "ok" | "err" | "warn", title: string, desc?: string) => void;
}

const POLICIES: { id: DeletePolicy; title: Key; desc: Key }[] = [
  { id: "versioned", title: "polA_t", desc: "polA_d" },
  { id: "mirror", title: "polB_t", desc: "polB_d" },
  { id: "addOnly", title: "polC_t", desc: "polC_d" },
];

const MODES: { id: TriggerMode; title: Key; desc: Key }[] = [
  { id: "manual", title: "modeManual_t", desc: "modeManual_d" },
  { id: "interval", title: "modeInterval_t", desc: "modeInterval_d" },
  { id: "realtime", title: "modeRealtime_t", desc: "modeRealtime_d" },
];

const THEMES: { id: Theme; label: Key }[] = [
  { id: "system", label: "thSystem" },
  { id: "dark", label: "thDark" },
  { id: "light", label: "thLight" },
];

const LANGS: { id: Language; label: Key }[] = [
  { id: "zh", label: "langZh" },
  { id: "en", label: "langEn" },
];

export default function SettingsView({ config, onSave, onToast }: Props) {
  const { t, setPreview: setLangPreview } = useI18n();
  const [draft, setDraft] = useState<Config>({ ...config });
  const [saving, setSaving] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    api
      .getAppInfo()
      .then(setInfo)
      .catch(() => setInfo(null));
  }, []);

  const dirty = JSON.stringify(draft) !== JSON.stringify(config);
  const set = <K extends keyof Config>(k: K, v: Config[K]) =>
    setDraft((d) => ({ ...d, [k]: v }));

  const save = async () => {
    setSaving(true);
    try {
      await onSave(draft);
    } finally {
      setSaving(false);
    }
  };

  const revert = () => {
    setDraft({ ...config });
    setLangPreview(null);
  };

  const pickLanguage = (lang: Language) => {
    set("language", lang);
    setLangPreview(lang); // 立即预览，保存后固化
  };

  const cleanup = async () => {
    setCleaning(true);
    try {
      const n = await api.cleanupVersions(null);
      onToast(
        "ok",
        t("cleanupDone"),
        t("cleanupDoneDesc", { n, days: draft.versionKeepDays }),
      );
    } catch (e) {
      onToast("err", t("cleanupFail"), String(e));
    } finally {
      setCleaning(false);
    }
  };

  return (
    <>
      <div className="toolbar">
        <h2>{t("settingsTitle")}</h2>
        <span className="hint">{t("settingsHint")}</span>
        <div className="spacer" />
        {dirty && (
          <span className="chip warn">
            <IconAlert width={13} height={13} /> {t("settingsDirty")}
          </span>
        )}
        <button className="btn" disabled={!dirty || saving} onClick={revert}>
          {t("btnRevert")}
        </button>
        <button
          className="btn btn-primary"
          disabled={!dirty || saving}
          onClick={() => void save()}
        >
          <IconCheck width={16} height={16} />{" "}
          {saving ? t("btnSaving") : t("btnSaveSettings")}
        </button>
      </div>

      <div className="section">
        <div className="section-title">{t("secPolicy")}</div>
        <div className="field" style={{ padding: "8px 0 18px" }}>
          <div className="option-grid" style={{ gridTemplateColumns: "1fr" }}>
            {POLICIES.map((p) => (
              <button
                key={p.id}
                className={`option-card ${draft.deletePolicy === p.id ? "sel" : ""}`}
                onClick={() => set("deletePolicy", p.id)}
              >
                <div className="opt-title">
                  <span className="radio-dot" />
                  {t(p.title)}
                </div>
                <div className="opt-desc">{t(p.desc)}</div>
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">{t("secTrigger")}</div>
        <div className="field" style={{ padding: "8px 0 4px" }}>
          <div className="option-grid" style={{ gridTemplateColumns: "1fr" }}>
            {MODES.map((m) => (
              <button
                key={m.id}
                className={`option-card ${draft.trigger.mode === m.id ? "sel" : ""}`}
                onClick={() =>
                  set("trigger", { ...draft.trigger, mode: m.id })
                }
              >
                <div className="opt-title">
                  <span className="radio-dot" />
                  {t(m.title)}
                </div>
                <div className="opt-desc">{t(m.desc)}</div>
              </button>
            ))}
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("intervalT")}</div>
            <div className="d">{t("intervalD")}</div>
          </div>
          <div className="ctl">
            <input
              className="input"
              type="number"
              min={1}
              max={1440}
              disabled={draft.trigger.mode !== "interval"}
              value={draft.trigger.intervalMinutes}
              onChange={(e) =>
                set("trigger", {
                  ...draft.trigger,
                  intervalMinutes: Math.max(1, Number(e.target.value) || 1),
                })
              }
            />
            <span className="muted">{t("unitMin")}</span>
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("debounceT")}</div>
            <div className="d">{t("debounceD")}</div>
          </div>
          <div className="ctl">
            <input
              className="input"
              type="number"
              min={0.2}
              max={600}
              step={0.5}
              disabled={draft.trigger.mode !== "realtime"}
              value={draft.trigger.debounceMs / 1000}
              onChange={(e) =>
                set("trigger", {
                  ...draft.trigger,
                  debounceMs: Math.round(
                    Math.min(600, Math.max(0.2, Number(e.target.value) || 0.2)) * 1000,
                  ),
                })
              }
            />
            <span className="muted">{t("unitSec")}</span>
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">{t("secBehavior")}</div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("previewFirstT")}</div>
            <div className="d">{t("previewFirstD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.confirmBeforeSync ? "on" : ""}`}
              onClick={() => set("confirmBeforeSync", !draft.confirmBeforeSync)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("notifyT")}</div>
            <div className="d">{t("notifyD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.notifyOnFinish ? "on" : ""}`}
              onClick={() => set("notifyOnFinish", !draft.notifyOnFinish)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("hashT")}</div>
            <div className="d">{t("hashD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.hashVerify ? "on" : ""}`}
              onClick={() => set("hashVerify", !draft.hashVerify)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("copyThreadsT")}</div>
            <div className="d">{t("copyThreadsD")}</div>
          </div>
          <div className="ctl">
            <input
              className="input"
              type="number"
              min={1}
              max={16}
              value={draft.copyThreads}
              onChange={(e) =>
                set(
                  "copyThreads",
                  Math.min(16, Math.max(1, Number(e.target.value) || 1)),
                )
              }
            />
            <span className="muted">{t("unitThreads")}</span>
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("verifyAfterCopyT")}</div>
            <div className="d">{t("verifyAfterCopyD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.verifyAfterCopy ? "on" : ""}`}
              onClick={() => set("verifyAfterCopy", !draft.verifyAfterCopy)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("diffScanT")}</div>
            <div className="d">{t("diffScanD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.diffScan ? "on" : ""}`}
              onClick={() => set("diffScan", !draft.diffScan)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("afterSyncT")}</div>
            <div className="d">{t("afterSyncD")}</div>
          </div>
          <div className="ctl">
            <select
              className="input"
              value={draft.afterSyncAction}
              onChange={(e) =>
                set("afterSyncAction", e.target.value as Config["afterSyncAction"])
              }
            >
              <option value="none">{t("afterSyncNone")}</option>
              <option value="script">{t("afterSyncScript")}</option>
              <option value="shutdown">{t("afterSyncShutdown")}</option>
            </select>
          </div>
        </div>

        {draft.afterSyncAction === "script" && (
          <div className="setting-row">
            <div className="info">
              <div className="t">{t("afterSyncScriptPathT")}</div>
              <div className="d">{t("afterSyncScriptPathD")}</div>
            </div>
            <div className="ctl">
              <input
                className="input"
                style={{ minWidth: 260 }}
                placeholder={t("afterSyncScriptPh")}
                value={draft.afterSyncScript}
                onChange={(e) => set("afterSyncScript", e.target.value)}
              />
            </div>
          </div>
        )}

        {draft.afterSyncAction === "shutdown" && (
          <div className="notice warn" style={{ marginTop: 4 }}>
            <span>{t("afterSyncShutdownWarn")}</span>
          </div>
        )}

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("autostartT")}</div>
            <div className="d">{t("autostartD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.autostart ? "on" : ""}`}
              onClick={() => set("autostart", !draft.autostart)}
            />
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">{t("secVersion")}</div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("versionOnReplaceT")}</div>
            <div className="d">{t("versionOnReplaceD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.versionOnReplace ? "on" : ""}`}
              onClick={() => set("versionOnReplace", !draft.versionOnReplace)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("versionOnDeleteT")}</div>
            <div className="d">{t("versionOnDeleteD")}</div>
          </div>
          <div className="ctl">
            <button
              className={`switch ${draft.versionOnDelete ? "on" : ""}`}
              onClick={() => set("versionOnDelete", !draft.versionOnDelete)}
            />
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("keepDaysT")}</div>
            <div className="d">{t("keepDaysD")}</div>
          </div>
          <div className="ctl">
            <input
              className="input"
              type="number"
              min={1}
              max={3650}
              value={draft.versionKeepDays}
              onChange={(e) =>
                set("versionKeepDays", Math.max(1, Number(e.target.value) || 1))
              }
            />
            <span className="muted">{t("unitDay")}</span>
            <button
              className="btn btn-sm"
              disabled={cleaning}
              onClick={() => void cleanup()}
            >
              {cleaning ? t("btnCleaning") : t("btnCleanNow")}
            </button>
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("versionMaxT")}</div>
            <div className="d">{t("versionMaxD")}</div>
          </div>
          <div className="ctl">
            <input
              className="input"
              type="number"
              min={0}
              max={10000}
              value={draft.versionMaxCount}
              onChange={(e) =>
                set(
                  "versionMaxCount",
                  Math.min(10000, Math.max(0, Number(e.target.value) || 0)),
                )
              }
            />
            <span className="muted">{t("unitCount")}</span>
          </div>
        </div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("versionTsT")}</div>
            <div className="d">{t("versionTsD")}</div>
          </div>
          <div className="ctl">
            <input
              className="input mono"
              style={{ minWidth: 200 }}
              value={draft.versionTimestampFormat}
              onChange={(e) => set("versionTimestampFormat", e.target.value)}
            />
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">{t("secAppearance")}</div>
        <div className="setting-row">
          <div className="info">
            <div className="t">{t("themeT")}</div>
            <div className="d">{t("themeD")}</div>
          </div>
          <div className="ctl">
            <div className="nav">
              {THEMES.map((th) => (
                <button
                  key={th.id}
                  className={`nav-btn ${draft.theme === th.id ? "active" : ""}`}
                  onClick={() => set("theme", th.id)}
                >
                  {t(th.label)}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">{t("secGeneral")}</div>

        <div className="setting-row">
          <div className="info">
            <div className="t">{t("langT")}</div>
            <div className="d">{t("langD")}</div>
          </div>
          <div className="ctl">
            <div className="nav">
              {LANGS.map((l) => (
                <button
                  key={l.id}
                  className={`nav-btn ${draft.language === l.id ? "active" : ""}`}
                  onClick={() => pickLanguage(l.id)}
                >
                  {t(l.label)}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">{t("secStorage")}</div>
        <div className="setting-row">
          <div className="info">
            <div className="t">
              {t("dataDirLabel", { path: info?.dataDir ?? "…" })}
            </div>
            <div className="d">{t("storageD")}</div>
            {info?.portable && (
              <div className="d" style={{ marginTop: 6 }}>
                <span className="chip warn">{t("portableBadge")}</span>{" "}
                {t("portableDesc")}
              </div>
            )}
            {info && (
              <div className="d mono" style={{ marginTop: 4, wordBreak: "break-all" }}>
                {t("exeLabel", { path: info.exePath })}
              </div>
            )}
          </div>
          <div className="ctl">
            <span className="chip">
              <IconSync width={13} height={13} /> {t("chipMirror")}
            </span>
          </div>
        </div>
      </div>

      <div className="section" style={{ paddingBottom: 16 }}>
        <div className="section-title">{t("secAbout")}</div>
        <div className="setting-row">
          <div className="info">
            <div className="t">{t("aboutT", { ver: info?.version ?? "0.1.0" })}</div>
            <div className="d">{t("aboutD")}</div>
          </div>
        </div>
      </div>
    </>
  );
}
