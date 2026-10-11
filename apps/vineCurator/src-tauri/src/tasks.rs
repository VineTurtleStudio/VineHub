use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const HASH_PROGRESS_EVERY: usize = 10;
const CHUNK: usize = 500;
const BUF_SIZE: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Serialize)]
pub struct HashStats {
    pub done: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HashProgress {
    /// hash | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub item_id: i64,
    pub rel_path: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub stats: HashStats,
}

impl HashProgress {
    pub fn failure(message: impl Into<String>) -> Self {
        HashProgress {
            phase: "finished".into(),
            index: 0,
            total: 0,
            item_id: 0,
            rel_path: String::new(),
            status: "error".into(),
            message: message.into(),
            stats: HashStats::default(),
        }
    }
}

#[derive(Debug)]
pub struct HashOutcome {
    pub stats: HashStats,
    pub status: String,
}

/// 把 `hash_state='none'` 的条目标为 `pending`；root_id 限定可选，item_ids 非空则只选这些。
/// 返回本次转为 pending 的行数。
pub fn mark_pending(
    conn: &Connection,
    root_id: Option<i64>,
    item_ids: &[i64],
) -> Result<usize, String> {
    let mut total = 0usize;
    for chunk in item_ids.chunks(CHUNK) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE items SET hash_state = 'pending'
             WHERE hash_state = 'none' AND id IN ({placeholders})"
        );
        let n = conn
            .execute(&sql, rusqlite::params_from_iter(chunk.iter().copied()))
            .map_err(|e| format!("标记 pending 失败: {e}"))?;
        total += n;
    }
    if item_ids.is_empty() {
        let n = match root_id {
            Some(rid) => conn
                .execute(
                    "UPDATE items SET hash_state = 'pending'
                     WHERE hash_state = 'none' AND root_id = ?1",
                    [rid],
                )
                .map_err(|e| format!("标记 pending 失败: {e}"))?,
            None => conn
                .execute(
                    "UPDATE items SET hash_state = 'pending' WHERE hash_state = 'none'",
                    [],
                )
                .map_err(|e| format!("标记 pending 失败: {e}"))?,
        };
        total += n;
    }
    Ok(total)
}

fn hash_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut buf = vec![0u8; BUF_SIZE];
    let mut hasher = Sha256::new();
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 流式 SHA-256 处理 pending 条目；64KB 分块；每 10 条节流进度。
/// 文件缺失 → 失败计数 + 回落 none（保持可重试，不阻塞队列）；取消中断不改 missing。
pub fn run_pending_hashes(
    conn: &Connection,
    root_path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&HashProgress),
) -> Result<HashOutcome, String> {
    let mut stats = HashStats::default();
    let pending: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, rel_path FROM items WHERE hash_state = 'pending' ORDER BY id")
            .map_err(|e| format!("查询 pending 失败: {e}"))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| format!("查询 pending 失败: {e}"))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| format!("查询 pending 失败: {e}"))?
    };
    let total = pending.len();

    progress(&HashProgress {
        phase: "hash".into(),
        index: 0,
        total,
        item_id: 0,
        rel_path: String::new(),
        status: "running".into(),
        message: "开始哈希…".into(),
        stats: stats.clone(),
    });

    let mut update = conn
        .prepare("UPDATE items SET hash = ?1, hash_state = 'done' WHERE id = ?2")
        .map_err(|e| format!("预编译 update 失败: {e}"))?;
    // 缺失文件回落 none（保持可重试，避免引入新枚举值）
    let mut fail = conn
        .prepare("UPDATE items SET hash_state = 'none' WHERE id = ?1")
        .map_err(|e| format!("预编译 fail 失败: {e}"))?;

    for (i, (item_id, rel_path)) in pending.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            progress(&HashProgress {
                phase: "finished".into(),
                index: i,
                total,
                item_id: *item_id,
                rel_path: rel_path.clone(),
                status: "cancelled".into(),
                message: format!("哈希已取消 · 完成 {} · 失败 {}", stats.done, stats.failed),
                stats: stats.clone(),
            });
            return Ok(HashOutcome {
                stats,
                status: "cancelled".into(),
            });
        }

        let abs = root_path.join(rel_path);
        match hash_file(&abs) {
            Ok(h) => {
                update
                    .execute(params![h, item_id])
                    .map_err(|e| format!("写入 hash 失败 id={item_id}: {e}"))?;
                stats.done += 1;
            }
            Err(e) => {
                fail.execute([item_id])
                    .map_err(|e| format!("标记 failed 失败 id={item_id}: {e}"))?;
                stats.failed += 1;
                progress(&HashProgress {
                    phase: "hash".into(),
                    index: i + 1,
                    total,
                    item_id: *item_id,
                    rel_path: rel_path.clone(),
                    status: "error".into(),
                    message: format!("{rel_path}: {e}"),
                    stats: stats.clone(),
                });
                continue;
            }
        }

        if (i + 1) % HASH_PROGRESS_EVERY == 0 || i + 1 == total {
            progress(&HashProgress {
                phase: "hash".into(),
                index: i + 1,
                total,
                item_id: *item_id,
                rel_path: rel_path.clone(),
                status: "running".into(),
                message: String::new(),
                stats: stats.clone(),
            });
        }
    }

    let message = format!("哈希完成 · 成功 {} · 失败 {}", stats.done, stats.failed);
    progress(&HashProgress {
        phase: "finished".into(),
        index: total,
        total,
        item_id: 0,
        rel_path: String::new(),
        status: "done".into(),
        message,
        stats: stats.clone(),
    });

    Ok(HashOutcome {
        stats,
        status: "done".into(),
    })
}

/// 单文件 SHA-256（测试/工具用）
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector_hello() {
        assert_eq!(
            hash_bytes(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
