import type { PairConfig, PairState } from "../types";
import { statusLabel, useI18n } from "../i18n";
import {
  IconArrowRight,
  IconEye,
  IconPencil,
  IconSync,
  IconTrash,
} from "./Icons";

interface Props {
  pair: PairConfig;
  state?: PairState;
  syncing: boolean;
  onSync: () => void;
  onPreview: () => void;
  onEdit: () => void;
  onToggle: (enabled: boolean) => void;
  onDelete: () => void;
}

export default function PairCard({
  pair,
  state,
  syncing,
  onSync,
  onPreview,
  onEdit,
  onToggle,
  onDelete,
}: Props) {
  const { t } = useI18n();
  const hasRun = Boolean(state?.lastSyncAt);
  const status = state?.lastStatus;
  const changes = hasRun
    ? (state?.added ?? 0) +
      (state?.updated ?? 0) +
      (state?.deleted ?? 0) +
      (state?.versioned ?? 0)
    : 0;

  return (
    <div className={`card ${pair.enabled ? "" : "disabled"}`}>
      <div className="card-head">
        <span className="card-title">{pair.name || t("cardUnnamed")}</span>
        <span className="spacer" />
        <button
          className={`switch ${pair.enabled ? "on" : ""}`}
          title={pair.enabled ? t("cardToggleOff") : t("cardToggleOn")}
          onClick={() => onToggle(!pair.enabled)}
        />
      </div>

      <div className="path-row">
        <span className="tag src">{t("tagSrc")}</span>
        <span className="path" title={pair.source}>
          {pair.source}
        </span>
      </div>
      <div className="path-row">
        <span className="tag dst">{t("tagDst")}</span>
        <span className="path" title={pair.dest}>
          {pair.dest}
        </span>
        <IconArrowRight width={14} height={14} className="muted" />
      </div>

      <div className="card-stats">
        {!hasRun && <span className="chip">{t("chipNeverSynced")}</span>}
        {hasRun && (
          <span
            className={`chip ${status === "ok" ? "ok" : status === "partial" ? "warn" : status === "skipped" ? "warn" : "err"}`}
          >
            {statusLabel(t, status)}
          </span>
        )}
        {hasRun && changes > 0 && (
          <>
            {(state?.added ?? 0) > 0 && (
              <span className="chip ok">
                {t("chipAdded")} <span className="n">{state?.added}</span>
              </span>
            )}
            {(state?.updated ?? 0) > 0 && (
              <span className="chip info">
                {t("chipUpdated")} <span className="n">{state?.updated}</span>
              </span>
            )}
            {(state?.deleted ?? 0) > 0 && (
              <span className="chip err">
                {t("chipDeleted")} <span className="n">{state?.deleted}</span>
              </span>
            )}
            {(state?.versioned ?? 0) > 0 && (
              <span className="chip warn">
                {t("chipVersion")} <span className="n">{state?.versioned}</span>
              </span>
            )}
          </>
        )}
        {hasRun && changes === 0 && status === "ok" && (
          <span className="chip">{t("chipNoChange")}</span>
        )}
        {(state?.failed ?? 0) > 0 && (
          <span className="chip err">
            {t("chipFailed")} <span className="n">{state?.failed}</span>
          </span>
        )}
        {(pair.include.length > 0 || pair.exclude.length > 0) && (
          <span className="chip">
            {t("filterLabel")}
            {pair.include.length > 0
              ? t("filterInc", { n: pair.include.length })
              : ""}
            {pair.exclude.length > 0
              ? t("filterExc", { n: pair.exclude.length })
              : ""}
          </span>
        )}
      </div>

      <div className="card-meta">
        {hasRun ? (
          <>
            <span>{t("cardLastSync", { time: state?.lastSyncAt ?? "" })}</span>
            {state?.message && (
              <span title={state.message}>· {state.message}</span>
            )}
          </>
        ) : (
          <span>{t("cardIdleHint")}</span>
        )}
      </div>

      <div className="card-actions">
        <button
          className="btn btn-primary btn-sm"
          disabled={syncing || !pair.enabled}
          onClick={onSync}
        >
          <IconSync width={14} height={14} /> {t("btnSync")}
        </button>
        <button
          className="btn btn-sm"
          disabled={syncing || !pair.enabled}
          onClick={onPreview}
        >
          <IconEye width={14} height={14} /> {t("btnPreview")}
        </button>
        <span className="spacer" />
        <button className="btn btn-ghost btn-sm" title={t("titleEdit")} onClick={onEdit}>
          <IconPencil width={15} height={15} />
        </button>
        <button
          className="btn btn-ghost btn-sm"
          title={t("titleDelete")}
          onClick={onDelete}
        >
          <IconTrash width={15} height={15} />
        </button>
      </div>
    </div>
  );
}
