use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const HEAD: u64 = 1024 * 1024;
const TAIL: u64 = 1024 * 1024;

/// 部分哈希：sha256(总长 LE || 头 1MiB || 尾 1MiB)。小文件顺序读全量（头尾不重叠）。
/// 同长度同头尾不同中间的文件会撞（设计取舍：只报告，确认仍靠完整哈希）。
pub fn compute_partial_hash(path: &Path, size: i64) -> Result<String, String> {
    if size <= 0 {
        return Err("空文件不参与重复检测".into());
    }
    let mut f = File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    let head_len = std::cmp::min(size as u64, HEAD);
    let mut head = vec![0u8; head_len as usize];
    f.read_exact(&mut head)
        .map_err(|e| format!("读取头失败: {e}"))?;
    hasher.update(&head);
    let remaining = size as u64 - head_len;
    if remaining > 0 {
        let tail_len = std::cmp::min(remaining, TAIL);
        f.seek(SeekFrom::End(-(tail_len as i64)))
            .map_err(|e| format!("定位尾部失败: {e}"))?;
        let mut tail = vec![0u8; tail_len as usize];
        f.read_exact(&mut tail)
            .map_err(|e| format!("读取尾失败: {e}"))?;
        hasher.update(&tail);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingItem {
    pub id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub size: i64,
}

/// 待补部分哈希：size 出现 ≥2 次（size 分组）、未缺失、尚无 partial_hash
pub fn pending_partial(conn: &Connection) -> Result<Vec<PendingItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, root_id, rel_path, size FROM items
             WHERE missing = 0 AND size > 0 AND partial_hash IS NULL
               AND size IN (
                 SELECT size FROM items WHERE missing = 0 AND size > 0
                 GROUP BY size HAVING COUNT(*) > 1
               )
             ORDER BY size DESC, id",
        )
        .map_err(|e| format!("查询候选失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PendingItem {
                id: r.get(0)?,
                root_id: r.get(1)?,
                rel_path: r.get(2)?,
                size: r.get(3)?,
            })
        })
        .map_err(|e| format!("查询候选失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取候选失败: {e}"))?);
    }
    Ok(out)
}

pub fn store_partial(conn: &Connection, id: i64, hash: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE items SET partial_hash = ?1 WHERE id = ?2",
        params![hash, id],
    )
    .map_err(|e| format!("写入部分哈希失败: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct DupeProgress {
    /// hash | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub item_id: i64,
    pub rel_path: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub hashed: usize,
    pub failed: usize,
}

impl DupeProgress {
    pub fn finished(hashed: usize, failed: usize) -> Self {
        Self {
            phase: "finished".into(),
            index: 0,
            total: 0,
            item_id: 0,
            rel_path: String::new(),
            status: "done".into(),
            message: format!("已计算 {hashed} 个部分哈希，失败 {failed}"),
            hashed,
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

/// 给可疑组（同 size）补部分哈希：只读原始文件，逐条写 partial_hash；取消在条目边界生效
pub fn run_partial_hashes(
    conn: &Connection,
    roots: &HashMap<i64, PathBuf>,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(&DupeProgress),
) -> Result<(), String> {
    let pending = pending_partial(conn)?;
    let total = pending.len();
    let mut hashed = 0usize;
    let mut failed = 0usize;
    for (i, p) in pending.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            emit(&DupeProgress {
                phase: "hash".into(),
                index: i,
                total,
                item_id: p.id,
                rel_path: p.rel_path.clone(),
                status: "cancelled".into(),
                message: format!("已取消：已计算 {hashed}，失败 {failed}"),
                hashed,
                failed,
            });
            return Ok(());
        }
        let path = match roots.get(&p.root_id) {
            Some(root) => root.join(&p.rel_path),
            None => {
                failed += 1;
                continue;
            }
        };
        match compute_partial_hash(&path, p.size) {
            Ok(h) => {
                store_partial(conn, p.id, &h)?;
                hashed += 1;
            }
            Err(_) => failed += 1,
        }
        if (i + 1) % 10 == 0 || i + 1 == total {
            emit(&DupeProgress {
                phase: "hash".into(),
                index: i + 1,
                total,
                item_id: p.id,
                rel_path: p.rel_path.clone(),
                status: "running".into(),
                message: String::new(),
                hashed,
                failed,
            });
        }
    }
    emit(&DupeProgress::finished(hashed, failed));
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct DupeItem {
    pub id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub hash_state: String,
    pub full_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DupeGroup {
    pub size: i64,
    pub count: usize,
    pub partial_hash: String,
    /// true = 组内完整哈希全部一致（确认重复）；false = 仅有 size + 部分哈希相同
    pub confirmed: bool,
    pub full_hash: Option<String>,
    pub items: Vec<DupeItem>,
}

/// 只读报告：(size, partial_hash) 相同且 ≥2 的组 → 组内再按完整哈希切分。
/// 完整哈希不同的互不入组（部分哈希撞车被剔除）；未计算完整哈希的归入未确认组。
pub fn report(conn: &Connection) -> Result<Vec<DupeGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT size, partial_hash, COUNT(*) FROM items
             WHERE partial_hash IS NOT NULL AND missing = 0 AND size > 0
             GROUP BY size, partial_hash HAVING COUNT(*) > 1
             ORDER BY size DESC",
        )
        .map_err(|e| format!("查询重复组失败: {e}"))?;
    let seeds: Vec<(i64, String)> = {
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| format!("查询重复组失败: {e}"))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row.map_err(|e| format!("读取重复组失败: {e}"))?);
        }
        v
    };

    let mut out = Vec::new();
    for (size, partial_hash) in seeds {
        let mut mstmt = conn
            .prepare(
                "SELECT id, root_id, rel_path, hash_state, hash FROM items
                 WHERE size = ?1 AND partial_hash = ?2 AND missing = 0
                 ORDER BY rel_path",
            )
            .map_err(|e| format!("查询组员失败: {e}"))?;
        let rows = mstmt
            .query_map(params![size, partial_hash], |r| {
                Ok(DupeItem {
                    id: r.get(0)?,
                    root_id: r.get(1)?,
                    rel_path: r.get(2)?,
                    hash_state: r.get(3)?,
                    full_hash: r.get(4)?,
                })
            })
            .map_err(|e| format!("查询组员失败: {e}"))?;
        let mut members = Vec::new();
        for row in rows {
            members.push(row.map_err(|e| format!("读取组员失败: {e}"))?);
        }

        let mut unknown: Vec<DupeItem> = Vec::new();
        let mut known: HashMap<String, Vec<DupeItem>> = HashMap::new();
        for it in members {
            match it.full_hash.clone() {
                Some(h) => known.entry(h).or_default().push(it),
                None => unknown.push(it),
            }
        }
        if unknown.len() >= 2 {
            out.push(DupeGroup {
                size,
                count: unknown.len(),
                partial_hash: partial_hash.clone(),
                confirmed: false,
                full_hash: None,
                items: unknown,
            });
        }
        for (h, mut items) in known {
            if items.len() >= 2 {
                items.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
                out.push(DupeGroup {
                    size,
                    count: items.len(),
                    partial_hash: partial_hash.clone(),
                    confirmed: true,
                    full_hash: Some(h),
                    items,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then(b.count.cmp(&a.count))
            .then(a.partial_hash.cmp(&b.partial_hash))
    });
    Ok(out)
}
