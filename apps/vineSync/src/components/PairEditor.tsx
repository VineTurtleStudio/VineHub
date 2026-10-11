import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import type { Config, FilterTestDto, PairConfig } from "../types";
import { useI18n } from "../i18n";
import { IconAlert, IconFolder, IconX } from "./Icons";

interface Props {
  mode: "new" | "edit";
  pair: PairConfig;
  config: Config;
  onClose: () => void;
  onSave: (config: Config) => Promise<boolean>;
}

async function pickFolder(title: string): Promise<string | null> {
  const dir = await open({ directory: true, multiple: false, title });
  if (typeof dir === "string") return dir;
  return null;
}

export default function PairEditor({ mode, pair, config, onClose, onSave }: Props) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<PairConfig>({ ...pair });
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [testPath, setTestPath] = useState("");
  const [testResult, setTestResult] = useState<FilterTestDto | null>(null);
  const [testing, setTesting] = useState(false);

  const set = <K extends keyof PairConfig>(k: K, v: PairConfig[K]) =>
    setDraft((d) => ({ ...d, [k]: v }));

  const joinList = (arr: string[]) => arr.join("\n");
  const parseList = (s: string) =>
    s
      .split("\n")
      .map((x) => x.trim())
      .filter(Boolean);

  const pick = async (field: "source" | "dest") => {
    try {
      const dir = await pickFolder(
        field === "source" ? t("pickSourceTitle") : t("pickDestTitle"),
      );
      if (dir) {
        set(field, dir);
        if (!draft.name.trim() && field === "source") {
          const seg = dir.split(/[\\/]/).filter(Boolean);
          set("name", seg[seg.length - 1] ?? "");
        }
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const runTest = async () => {
    if (!testPath.trim()) return;
    setTesting(true);
    try {
      setTestResult(
        await api.testFilter(draft.include, draft.exclude, testPath.trim()),
      );
    } catch (e) {
      setTestResult(null);
      setError(String(e));
    } finally {
      setTesting(false);
    }
  };

  const submit = async () => {
    setError(null);
    if (!draft.source.trim() || !draft.dest.trim()) {
      setError(t("editorPickDirs"));
      return;
    }
    const pairs =
      mode === "new"
        ? [...config.pairs, draft]
        : config.pairs.map((p) => (p.id === draft.id ? draft : p));
    setSaving(true);
    try {
      await onSave({ ...config, pairs });
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal wide" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h3>{mode === "new" ? t("editorTitleNew") : t("editorTitleEdit")}</h3>
          <span className="spacer" />
          <button className="icon-btn" onClick={onClose}>
            <IconX width={16} height={16} />
          </button>
        </div>

        <div className="modal-body">
          {error && (
            <div className="form-error">
              <IconAlert width={16} height={16} style={{ flex: "0 0 auto" }} />
              <span>{error}</span>
            </div>
          )}

          <div className="field">
            <label>{t("labelName")}</label>
            <input
              className="input"
              placeholder={t("phName")}
              value={draft.name}
              onChange={(e) => set("name", e.target.value)}
            />
          </div>

          <div className="field">
            <label>{t("labelSource")}</label>
            <div className="input-row">
              <input
                className="input mono"
                placeholder={t("phSource")}
                value={draft.source}
                onChange={(e) => set("source", e.target.value)}
              />
              <button className="btn" onClick={() => void pick("source")}>
                <IconFolder width={15} height={15} /> {t("btnBrowse")}
              </button>
            </div>
          </div>

          <div className="field">
            <label>{t("labelDest")}</label>
            <div className="input-row">
              <input
                className="input mono"
                placeholder={t("phDest")}
                value={draft.dest}
                onChange={(e) => set("dest", e.target.value)}
              />
              <button className="btn" onClick={() => void pick("dest")}>
                <IconFolder width={15} height={15} /> {t("btnBrowse")}
              </button>
            </div>
          </div>

          <div className="field">
            <label>{t("labelPairInterval")}</label>
            <div className="input-row">
              <input
                className="input"
                type="number"
                min={1}
                max={10080}
                placeholder={t("phPairInterval")}
                value={draft.intervalMinutes ?? ""}
                onChange={(e) =>
                  set(
                    "intervalMinutes",
                    e.target.value === ""
                      ? null
                      : Math.min(10080, Math.max(1, Number(e.target.value) || 1)),
                  )
                }
              />
              <span className="muted">{t("unitMin")}</span>
            </div>
            <div className="help">{t("helpPairInterval")}</div>
          </div>

          <div className="field">
            <label>{t("labelInclude")}</label>
            <textarea
              className="input"
              spellCheck={false}
              placeholder={"*.docx\n项目A/**"}
              value={joinList(draft.include)}
              onChange={(e) => set("include", parseList(e.target.value))}
            />
            <div className="help">{t("helpInclude")}</div>
          </div>

          <div className="field">
            <label>{t("labelExclude")}</label>
            <textarea
              className="input"
              spellCheck={false}
              placeholder={"*.tmp\nnode_modules/**\n$RECYCLE.BIN/**"}
              value={joinList(draft.exclude)}
              onChange={(e) => set("exclude", parseList(e.target.value))}
            />
            <div className="help">{t("helpExclude")}</div>
          </div>

          <div className="field">
            <label>{t("labelFilterTest")}</label>
            <div className="input-row">
              <input
                className="input mono"
                placeholder={t("phFilterTest")}
                value={testPath}
                onChange={(e) => {
                  setTestPath(e.target.value);
                  setTestResult(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void runTest();
                }}
              />
              <button
                className="btn"
                onClick={() => void runTest()}
                disabled={testing || !testPath.trim()}
              >
                {t("btnTestFilter")}
              </button>
            </div>
            {testResult && (
              <div style={{ marginTop: 6 }}>
                <span className={`chip ${testResult.matched ? "ok" : "err"}`}>
                  {testResult.reason}
                </span>
              </div>
            )}
          </div>
        </div>

        <div className="modal-foot">
          <button className="btn" onClick={onClose} disabled={saving}>
            {t("btnCancel")}
          </button>
          <button
            className="btn btn-primary"
            onClick={() => void submit()}
            disabled={saving}
          >
            {saving ? t("btnSaving") : t("btnSave")}
          </button>
        </div>
      </div>
    </div>
  );
}
