import { useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import type { LogRow } from "../types";
import { statusLabel, triggerLabel, useI18n } from "../i18n";
import { statusClass } from "../utils";
import { IconHistory, IconSync, IconTrash } from "./Icons";

interface Props {
  rows: LogRow[];
  onRefresh: () => void;
  onClear: () => void;
}

export default function HistoryView({ rows, onRefresh, onClear }: Props) {
  const { t } = useI18n();
  const [exporting, setExporting] = useState(false);
  const [exportErr, setExportErr] = useState<string | null>(null);

  const onExport = async () => {
    setExportErr(null);
    try {
      const now = new Date();
      const pad = (n: number) => String(n).padStart(2, "0");
      const name = `vinesync-log-${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}.csv`;
      const path = await save({
        defaultPath: name,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!path) return;
      setExporting(true);
      await api.exportHistory(path, rows);
    } catch (e) {
      setExportErr(String(e));
    } finally {
      setExporting(false);
    }
  };

  return (
    <>
      <div className="toolbar">
        <h2>{t("historyTitle")}</h2>
        <span className="hint">
          {rows.length > 0 ? t("historyCount", { n: rows.length }) : t("historyHint")}
        </span>
        <div className="spacer" />
        <div className="history-actions">
          <button className="btn btn-sm" onClick={onRefresh}>
            <IconSync width={14} height={14} /> {t("btnRefresh")}
          </button>
          <button
            className="btn btn-sm"
            disabled={rows.length === 0 || exporting}
            onClick={() => void onExport()}
          >
            {exporting ? t("btnExporting") : t("btnExport")}
          </button>
          <button
            className="btn btn-sm btn-danger"
            disabled={rows.length === 0}
            onClick={onClear}
          >
            <IconTrash width={14} height={14} /> {t("btnClearHistory")}
          </button>
        </div>
      </div>

      {exportErr && (
        <div className="hint" style={{ color: "var(--danger)", marginBottom: 8 }}>
          {t("exportFailed")}: {exportErr}
        </div>
      )}

      {rows.length === 0 ? (
        <div className="empty">
          <div className="big">
            <IconHistory
              width={26}
              height={26}
              style={{ verticalAlign: "middle", marginRight: 8 }}
            />
            {t("historyEmpty")}
          </div>
          <p>{t("historyEmptyDesc")}</p>
        </div>
      ) : (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>{t("thTime")}</th>
                <th>{t("thPair")}</th>
                <th>{t("thTrigger")}</th>
                <th>{t("thStatus")}</th>
                <th>{t("thAdded")}</th>
                <th>{t("thUpdated")}</th>
                <th>{t("thDeleted")}</th>
                <th>{t("thVersion")}</th>
                <th>{t("thFailed")}</th>
                <th>{t("thMessage")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id}>
                  <td className="mono">{r.startedAt}</td>
                  <td className="strong">{r.pairName}</td>
                  <td>{triggerLabel(t, r.trigger)}</td>
                  <td>
                    <span className={`status-tag ${statusClass(r.status)}`}>
                      {statusLabel(t, r.status)}
                    </span>
                  </td>
                  <td className="num">{r.added || "–"}</td>
                  <td className="num">{r.updated || "–"}</td>
                  <td className="num">{r.deleted || "–"}</td>
                  <td className="num">{r.versioned || "–"}</td>
                  <td className="num" style={{ color: r.failed ? "var(--danger)" : undefined }}>
                    {r.failed || "–"}
                  </td>
                  <td title={r.message}>
                    <span
                      style={{
                        display: "inline-block",
                        maxWidth: 320,
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {r.message}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="legend">
        <span>{t("legendTrigger")}</span>
        <span>{t("legendStatus")}</span>
      </div>
    </>
  );
}
