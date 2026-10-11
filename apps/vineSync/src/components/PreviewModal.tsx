import { useEffect, useRef, useState, type UIEvent } from "react";
import type { Preview } from "../types";
import { formatBytes } from "../utils";
import { kindLabel, useI18n } from "../i18n";
import { IconAlert, IconCheck, IconSync, IconX } from "./Icons";

interface Props {
  preview: Preview;
  syncing: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

/** 超过该行数启用虚拟滚动（窗口化渲染） */
const VIRTUALIZE_AT = 200;
const OVERSCAN = 12;

export default function PreviewModal({ preview, syncing, onCancel, onConfirm }: Props) {
  const { t } = useI18n();
  const c = preview.counts;
  const total = c.added + c.updated + c.deleted + c.versioned;
  const canConfirm = total > 0 && !preview.targetUnavailable;

  const listRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(480);
  const [rowH, setRowH] = useState(38);

  const changes = preview.changes;
  const virtualize = changes.length > VIRTUALIZE_AT;
  const start = virtualize
    ? Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN)
    : 0;
  const end = virtualize
    ? Math.min(changes.length, Math.ceil((scrollTop + viewportH) / rowH) + OVERSCAN)
    : changes.length;

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (el.clientHeight > 0) setViewportH(el.clientHeight);
    const first = el.querySelector<HTMLElement>(".change-row");
    if (first) {
      const h = first.getBoundingClientRect().height;
      if (h > 0) setRowH(h);
    }
  }, [changes.length]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    setScrollTop(el.scrollTop);
    if (Math.abs(el.clientHeight - viewportH) > 4) setViewportH(el.clientHeight);
  };

  const rows = changes.slice(start, end);

  return (
    <div className="modal-backdrop" onMouseDown={onCancel}>
      <div className="modal wide" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <IconSync width={18} height={18} style={{ color: "var(--accent)" }} />
          <h3>{t("previewTitle")}</h3>
          <span className="spacer" />
          <button className="icon-btn" onClick={onCancel}>
            <IconX width={16} height={16} />
          </button>
        </div>

        <div className="modal-body">
          {preview.targetUnavailable && (
            <div className="notice warn">
              <IconAlert width={16} height={16} style={{ flex: "0 0 auto" }} />
              <span>{t("previewWarnTarget")}</span>
            </div>
          )}

          {preview.errors.length > 0 && (
            <div className="notice err">
              <IconAlert width={16} height={16} style={{ flex: "0 0 auto" }} />
              <div>
                {preview.errors.map((e, i) => (
                  <div key={i}>{e}</div>
                ))}
              </div>
            </div>
          )}

          <div className="summary-row">
            <span className="chip ok">{t("sumAdded", { n: c.added })}</span>
            <span className="chip info">{t("sumUpdated", { n: c.updated })}</span>
            <span className="chip err">{t("sumDeleted", { n: c.deleted })}</span>
            <span className="chip warn">{t("sumVersioned", { n: c.versioned })}</span>
            {preview.suppressed > 0 && (
              <span className="chip">{t("sumSuppressed", { n: preview.suppressed })}</span>
            )}
            <span className="chip">
              {t("sumTransfer", { bytes: formatBytes(preview.totalBytes) })}
            </span>
          </div>

          {preview.suppressed > 0 && (
            <div className="notice info">
              <IconAlert width={16} height={16} style={{ flex: "0 0 auto" }} />
              <span>{t("suppressedNotice", { n: preview.suppressed })}</span>
            </div>
          )}

          {total === 0 ? (
            <div className="notice info" style={{ marginBottom: 0 }}>
              <IconCheck width={16} height={16} style={{ flex: "0 0 auto" }} />
              <span>{t("previewNoChanges")}</span>
            </div>
          ) : (
            <>
              {preview.changes.length < total && (
                <div className="notice info" style={{ marginTop: 4 }}>
                  <IconAlert width={16} height={16} style={{ flex: "0 0 auto" }} />
                  <span>
                    {t("previewTruncated", { shown: preview.changes.length, total })}
                  </span>
                </div>
              )}
              <div className="change-list" ref={listRef} onScroll={onScroll}>
                {virtualize && start > 0 && (
                  <div style={{ height: start * rowH }} aria-hidden />
                )}
                {rows.map((ch, i) => (
                  <div
                    className="change-row"
                    key={start + i}
                    style={
                      (start + i) % 2 === 1
                        ? { background: "var(--card-2)" }
                        : { background: "var(--card)" }
                    }
                  >
                    <span className={`kind ${ch.kind}`}>{kindLabel(t, ch.kind)}</span>
                    <span className="pair" title={ch.pairName}>
                      {ch.pairName}
                    </span>
                    <span className="rel" title={ch.relPath}>
                      {ch.relPath}
                    </span>
                    <span className="size">
                      {ch.kind === "delete" || ch.kind === "versioned"
                        ? "—"
                        : formatBytes(ch.size)}
                    </span>
                  </div>
                ))}
                {virtualize && end < changes.length && (
                  <div style={{ height: (changes.length - end) * rowH }} aria-hidden />
                )}
              </div>
            </>
          )}
        </div>

        <div className="modal-foot">
          <button className="btn" onClick={onCancel}>
            {t("btnCancel")}
          </button>
          <button
            className="btn btn-primary"
            disabled={!canConfirm || syncing}
            onClick={onConfirm}
          >
            <IconCheck width={16} height={16} />
            {syncing ? t("btnSyncing") : t("btnConfirmSync", { n: total })}
          </button>
        </div>
      </div>
    </div>
  );
}
