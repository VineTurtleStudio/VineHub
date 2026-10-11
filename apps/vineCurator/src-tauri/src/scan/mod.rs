pub mod classify;

use classify::{classify_type, ext_table};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

const CHUNK: usize = 500;
const WALK_PROGRESS_EVERY: u64 = 1000;
const MAX_DEPTH: usize = 24;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

const UPSERT_SQL: &str = "
INSERT INTO items (root_id, rel_path, file_name, ext, type, size, mtime, missing)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
ON CONFLICT(root_id, rel_path) DO UPDATE SET
  hash = CASE WHEN items.size != excluded.size OR items.mtime != excluded.mtime
              THEN NULL ELSE items.hash END,
  hash_state = CASE WHEN items.size != excluded.size OR items.mtime != excluded.mtime
                    THEN 'none' ELSE items.hash_state END,
  meta_state = CASE WHEN items.size != excluded.size OR items.mtime != excluded.mtime
                    THEN 'none' ELSE items.meta_state END,
  file_name = excluded.file_name,
  ext = excluded.ext,
  type = excluded.type,
  size = excluded.size,
  mtime = excluded.mtime,
  missing = 0,
  updated_at = datetime('now')
WHERE items.size != excluded.size
   OR items.mtime != excluded.mtime
   OR items.missing != 0
   OR items.file_name != excluded.file_name
   OR items.ext != excluded.ext
   OR items.type != excluded.type
RETURNING id";

#[derive(Debug, Clone)]
struct FileEntry {
    rel_path: String,
    file_name: String,
    ext: String,
    type_name: &'static str,
    size: i64,
    mtime: i64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ScanStats {
    pub seen: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub missing: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    /// walk | finalize | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rel_path: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub stats: ScanStats,
}

impl ScanProgress {
    pub fn failure(message: impl Into<String>) -> Self {
        ScanProgress {
            phase: "finished".into(),
            index: 0,
            total: 0,
            rel_path: String::new(),
            status: "error".into(),
            message: message.into(),
            stats: ScanStats::default(),
        }
    }
}

#[derive(Debug)]
pub struct ScanOutcome {
    pub stats: ScanStats,
    /// done | cancelled
    pub status: String,
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse(_meta: &std::fs::Metadata) -> bool {
    false
}

fn is_skipped_name(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "thumbs.db" | "desktop.ini" | "ehthumbs.db" | "$recycle.bin" | "system volume information"
    )
}

/// 迭代遍历（防 symlink/junction 环、深度上限）；on_file 返回 false 提前停止。
/// 仅根目录不可读返回 Err；子目录/文件不可读降级计数继续。
fn walk(
    root: &Path,
    cancel: &AtomicBool,
    errors: &mut u64,
    mut on_file: impl FnMut(FileEntry) -> bool,
) -> Result<usize, String> {
    let root_meta =
        std::fs::metadata(root).map_err(|e| format!("根目录不可访问 {}: {e}", root.display()))?;
    if !root_meta.is_dir() {
        return Err(format!("根目录不是目录: {}", root.display()));
    }
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut count: usize = 0;
    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(count);
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                *errors += 1;
                eprintln!("[scan] 目录不可读 {}: {e}", dir.display());
                continue;
            }
        };
        for entry in rd {
            if cancel.load(Ordering::Relaxed) {
                return Ok(count);
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    *errors += 1;
                    eprintln!("[scan] 目录项不可读: {e}");
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_skipped_name(&name) {
                continue;
            }
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    *errors += 1;
                    eprintln!("[scan] 元数据不可读 {}: {e}", path.display());
                    continue;
                }
            };
            if is_reparse(&meta) {
                continue;
            }
            if meta.is_dir() {
                if depth + 1 > MAX_DEPTH {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if meta.is_file() {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let fe = FileEntry {
                    rel_path: rel.to_string_lossy().replace('\\', "/"),
                    file_name: name.clone(),
                    type_name: classify_type(&ext),
                    ext,
                    size: meta.len() as i64,
                    mtime: mtime_ms(&meta),
                };
                if !on_file(fe) {
                    return Ok(count);
                }
                count += 1;
            }
        }
    }
    Ok(count)
}

fn flush_batch(
    conn: &mut Connection,
    root_id: i64,
    batch: &mut Vec<FileEntry>,
    stats: &mut ScanStats,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<(), String> {
        let mut upsert = tx
            .prepare(UPSERT_SQL)
            .map_err(|e| format!("预编译 upsert 失败: {e}"))?;
        let mut seen_ins = tx
            .prepare("INSERT OR IGNORE INTO scan_seen (rel_path) VALUES (?1)")
            .map_err(|e| format!("预编译 seen 失败: {e}"))?;
        for fe in batch.drain(..) {
            match upsert.query_row(
                params![
                    root_id,
                    fe.rel_path,
                    fe.file_name,
                    fe.ext,
                    fe.type_name,
                    fe.size,
                    fe.mtime
                ],
                |r| r.get::<_, i64>(0),
            ) {
                Ok(id) => {
                    stats.changed += 1;
                    if let Some(table) = ext_table(fe.type_name) {
                        if let Err(e) = tx.execute(
                            &format!("INSERT OR IGNORE INTO {table} (item_id) VALUES (?1)"),
                            [id],
                        ) {
                            stats.errors += 1;
                            eprintln!("[scan] 扩展表占位失败 id={id}: {e}");
                        }
                    }
                    // FTS5 显式同步（无 trigger，同事务）
                    if let Err(e) = crate::fts::sync_item(&tx, id) {
                        stats.errors += 1;
                        eprintln!("[scan] FTS 同步失败 id={id}: {e}");
                    }
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    stats.unchanged += 1;
                }
                Err(e) => {
                    stats.errors += 1;
                    eprintln!("[scan] upsert 失败 {}: {e}", fe.rel_path);
                }
            }
            if let Err(e) = seen_ins.execute([&fe.rel_path]) {
                return Err(format!("记录已见条目失败: {e}"));
            }
        }
        Ok(())
    })();
    match inner {
        Ok(()) => tx.commit().map_err(|e| format!("提交失败: {e}")),
        Err(e) => Err(e),
    }
}

/// 扫描一个根目录：遍历 + 批量 upsert + 完整走完后标记缺失。
/// 指纹 = size + mtime(ms)；未变零写入；变化时懒哈希失效（hash_state → none）。
/// 取消/中断不标 missing（断点续扫安全）。
pub fn scan_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanOutcome, String> {
    let mut stats = ScanStats::default();
    let mut batch: Vec<FileEntry> = Vec::with_capacity(CHUNK);
    let mut walk_errors: u64 = 0;
    let mut flush_error: Option<String> = None;

    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS scan_seen (rel_path TEXT PRIMARY KEY) WITHOUT ROWID;
         DELETE FROM scan_seen;",
    )
    .map_err(|e| format!("初始化扫描临时表失败: {e}"))?;

    progress(&ScanProgress {
        phase: "walk".into(),
        index: 0,
        total: 0,
        rel_path: String::new(),
        status: "running".into(),
        message: "开始扫描".into(),
        stats: stats.clone(),
    });

    let walk_res = walk(root_path, cancel, &mut walk_errors, |fe| {
        stats.seen += 1;
        if stats.seen % WALK_PROGRESS_EVERY == 0 {
            progress(&ScanProgress {
                phase: "walk".into(),
                index: stats.seen as usize,
                total: 0,
                rel_path: fe.rel_path.clone(),
                status: "running".into(),
                message: "遍历中…".into(),
                stats: stats.clone(),
            });
        }
        batch.push(fe);
        if batch.len() >= CHUNK {
            if let Err(e) = flush_batch(conn, root_id, &mut batch, &mut stats) {
                flush_error = Some(e);
                return false;
            }
        }
        true
    });

    stats.errors += walk_errors;

    if let Some(e) = flush_error {
        return Err(e);
    }
    let walked = walk_res?;

    if !batch.is_empty() {
        flush_batch(conn, root_id, &mut batch, &mut stats)?;
    }

    if cancel.load(Ordering::Relaxed) {
        let message = format!(
            "扫描已取消 · 变更 {} · 跳过 {} · 已见 {}",
            stats.changed, stats.unchanged, stats.seen
        );
        progress(&ScanProgress {
            phase: "finished".into(),
            index: walked,
            total: walked,
            rel_path: String::new(),
            status: "cancelled".into(),
            message,
            stats: stats.clone(),
        });
        return Ok(ScanOutcome {
            stats,
            status: "cancelled".into(),
        });
    }

    progress(&ScanProgress {
        phase: "finalize".into(),
        index: walked,
        total: walked,
        rel_path: String::new(),
        status: "running".into(),
        message: "标记缺失条目…".into(),
        stats: stats.clone(),
    });

    let marked = conn
        .execute(
            "UPDATE items SET missing = 1, updated_at = datetime('now')
             WHERE root_id = ?1 AND missing = 0
               AND NOT EXISTS (SELECT 1 FROM scan_seen s WHERE s.rel_path = items.rel_path)",
            [root_id],
        )
        .map_err(|e| format!("标记缺失失败: {e}"))?;
    stats.missing = marked as u64;

    let message = format!(
        "扫描完成 · 变更 {} · 跳过 {} · 缺失 {} · 错误 {}",
        stats.changed, stats.unchanged, stats.missing, stats.errors
    );
    progress(&ScanProgress {
        phase: "finished".into(),
        index: walked,
        total: walked,
        rel_path: String::new(),
        status: "done".into(),
        message,
        stats: stats.clone(),
    });

    Ok(ScanOutcome {
        stats,
        status: "done".into(),
    })
}
