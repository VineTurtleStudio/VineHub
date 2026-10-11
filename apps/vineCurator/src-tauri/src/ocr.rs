//! OCR 队列（纯逻辑，零 Tauri）：book PDF → OCRmyPDF 派生 PDF + sidecar 文本 →
//! `book_items.ocr_text` 入库 + FTS 重建。**原始媒体只读**：ocrmypdf 只写派生目录
//! `<derived>/ocr/<item_id>-<mtime_ms>.{pdf,txt}`。
//! `ocr_status` 状态机（0002 CHECK）：none → queued → done | failed（failed 可重试；
//! done 但 ocr_mtime ≠ items.mtime 的指纹失配自动重跑）。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fts;
use crate::thumbs;

/// ocr_text 入库上限（字符，按 char 截断）：保护 DB/FTS 体积
const OCR_TEXT_CAP: usize = 200_000;

pub trait OcrRunner: Send {
    fn ocr(&self, input: &Path, out_pdf: &Path, sidecar: &Path, lang: &str) -> Result<(), String>;
}

pub struct OcrmypdfRunner {
    pub program: String,
}

impl OcrmypdfRunner {
    pub fn from_overrides(overrides: &HashMap<String, String>) -> Self {
        let configured = overrides
            .get("ocrmypdf_path")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        Self {
            program: configured.unwrap_or("ocrmypdf").to_string(),
        }
    }
}

impl OcrRunner for OcrmypdfRunner {
    fn ocr(&self, input: &Path, out_pdf: &Path, sidecar: &Path, lang: &str) -> Result<(), String> {
        let mut cmd = Command::new(&self.program);
        cmd.arg("--quiet");
        let lang = lang.trim();
        if !lang.is_empty() {
            cmd.arg("-l").arg(lang);
        }
        cmd.arg("--sidecar")
            .arg(sidecar)
            .arg(input)
            .arg(out_pdf)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let out = cmd
            .output()
            .map_err(|e| format!("启动 ocrmypdf 失败（检查 ocrmypdf_path / tesseract）: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let tail: String = String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(400)
                .collect();
            Err(format!("ocrmypdf 退出 {:?}: {tail}", out.status.code()))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingOcr {
    pub id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub mtime: i64,
}

/// 待 OCR：book + pdf、未缺失；状态 none/failed，或 done 但 mtime 失配（文件已变）
pub fn pending_ocr(conn: &Connection, root_scope: Option<i64>) -> Result<Vec<PendingOcr>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.root_id, i.rel_path, i.mtime
             FROM items i JOIN book_items b ON b.item_id = i.id
             WHERE i.missing = 0 AND i.type = 'book' AND lower(i.ext) = 'pdf'
               AND (?1 IS NULL OR i.root_id = ?1)
               AND (b.ocr_status IN ('none','failed')
                    OR (b.ocr_status = 'done'
                        AND (b.ocr_mtime IS NULL OR b.ocr_mtime <> i.mtime)))
             ORDER BY i.id",
        )
        .map_err(|e| format!("查询 OCR 候选失败: {e}"))?;
    let rows = stmt
        .query_map(params![root_scope], |r| {
            Ok(PendingOcr {
                id: r.get(0)?,
                root_id: r.get(1)?,
                rel_path: r.get(2)?,
                mtime: r.get(3)?,
            })
        })
        .map_err(|e| format!("查询 OCR 候选失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取 OCR 候选失败: {e}"))?);
    }
    Ok(out)
}

pub fn count_pending(conn: &Connection, root_scope: Option<i64>) -> Result<i64, String> {
    let n = pending_ocr(conn, root_scope)?.len();
    Ok(n as i64)
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrProgress {
    /// queue | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub item_id: i64,
    pub rel_path: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub done: usize,
    pub failed: usize,
}

impl OcrProgress {
    pub fn finished(done: usize, failed: usize) -> Self {
        Self {
            phase: "finished".into(),
            index: 0,
            total: 0,
            item_id: 0,
            rel_path: String::new(),
            status: "done".into(),
            message: format!("OCR 完成 {done} 个，失败 {failed}"),
            done,
            failed,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            phase: "finished".into(),
            status: "error".into(),
            message: message.into(),
            ..Self::finished(0, 0)
        }
    }
}

fn ocr_dir(conn: &Connection, default_derived: &Path) -> Result<PathBuf, String> {
    let dir = thumbs::derived_base(conn, default_derived)?.join("ocr");
    fs::create_dir_all(&dir).map_err(|e| format!("创建 OCR 派生目录失败: {e}"))?;
    Ok(dir)
}

/// 清掉同一 item 的旧 mtime 派生文件（保留 current stem 的 pdf/txt）
fn cleanup_stale(dir: &Path, item_id: i64, keep_stem: &str) {
    let prefix = format!("{item_id}-");
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem != keep_stem && (name.ends_with(".pdf") || name.ends_with(".txt")) {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn cap_text(text: String) -> String {
    if text.chars().count() > OCR_TEXT_CAP {
        text.chars().take(OCR_TEXT_CAP).collect()
    } else {
        text
    }
}

/// 队列执行：逐条 claim（queued）→ runner 输出派生 pdf + sidecar → ocr_text 入库 +
/// FTS 同步 → done/failed；取消在条目边界生效（claim 前检查，取消零写入）。
#[allow(clippy::too_many_arguments)]
pub fn run_ocr_queue(
    conn: &Connection,
    roots: &HashMap<i64, PathBuf>,
    default_derived: &Path,
    runner: &dyn OcrRunner,
    lang: &str,
    root_scope: Option<i64>,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(&OcrProgress),
) -> Result<(), String> {
    let pending = pending_ocr(conn, root_scope)?;
    let total = pending.len();
    let dir = ocr_dir(conn, default_derived)?;
    let mut done = 0usize;
    let mut failed = 0usize;
    for (i, p) in pending.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            emit(&OcrProgress {
                phase: "queue".into(),
                index: i,
                total,
                item_id: p.id,
                rel_path: p.rel_path.clone(),
                status: "cancelled".into(),
                message: format!("已取消：完成 {done}，失败 {failed}"),
                done,
                failed,
            });
            return Ok(());
        }
        emit(&OcrProgress {
            phase: "queue".into(),
            index: i + 1,
            total,
            item_id: p.id,
            rel_path: p.rel_path.clone(),
            status: "running".into(),
            message: "OCR 中…".into(),
            done,
            failed,
        });
        let Some(root) = roots.get(&p.root_id) else {
            failed += 1;
            continue;
        };
        let src = root.join(&p.rel_path);
        if !src.is_file() {
            failed += 1;
            continue;
        }
        conn.execute(
            "UPDATE book_items SET ocr_status = 'queued' WHERE item_id = ?1",
            params![p.id],
        )
        .map_err(|e| format!("标记 queued 失败 id={}: {e}", p.id))?;
        let stem = format!("{}-{}", p.id, p.mtime);
        let out_pdf = dir.join(format!("{stem}.pdf"));
        let sidecar = dir.join(format!("{stem}.txt"));
        match runner.ocr(&src, &out_pdf, &sidecar, lang) {
            Ok(()) => {
                let text = fs::read_to_string(&sidecar).unwrap_or_default();
                let capped = cap_text(text);
                conn.execute(
                    "UPDATE book_items SET ocr_text = ?1, ocr_mtime = ?2, ocr_status = 'done'
                     WHERE item_id = ?3",
                    params![capped, p.mtime, p.id],
                )
                .map_err(|e| format!("写入 OCR 结果失败 id={}: {e}", p.id))?;
                fts::sync_item(conn, p.id)?;
                cleanup_stale(&dir, p.id, &stem);
                done += 1;
            }
            Err(e) => {
                conn.execute(
                    "UPDATE book_items SET ocr_status = 'failed' WHERE item_id = ?1",
                    params![p.id],
                )
                .map_err(|e2| format!("标记 failed 失败 id={}: {e2}", p.id))?;
                failed += 1;
                emit(&OcrProgress {
                    phase: "queue".into(),
                    index: i + 1,
                    total,
                    item_id: p.id,
                    rel_path: p.rel_path.clone(),
                    status: "running".into(),
                    message: e,
                    done,
                    failed,
                });
            }
        }
    }
    emit(&OcrProgress::finished(done, failed));
    Ok(())
}

/// 单条当前 ocr 状态查询（详情面板可复用）
pub fn ocr_status_of(conn: &Connection, item_id: i64) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT ocr_status FROM book_items WHERE item_id = ?1",
        params![item_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| format!("查询 ocr_status 失败: {e}"))
}
