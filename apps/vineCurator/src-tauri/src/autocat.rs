//! 目录结构镜像自动分类（纯逻辑，零 Tauri）：取条目父目录相对根的前 N 层文件夹
//! 作为物化分类路径，以 `source='auto'` 关联。重跑幂等：desired 之外的 auto 关联
//! 删除，manual/rule 关联永不触碰。depth 由调用方读 settings `auto_cat_depth`
//! （1..=5，0 = 关闭）；进度事件由调用方 emit（`autocat-progress`）。

use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::categories;

#[derive(Debug, Clone, Default, Serialize)]
pub struct AutoCatStats {
    pub items: usize,
    pub linked: usize,
    pub unlinked: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoCatProgress {
    /// queue | finished
    pub phase: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub stats: AutoCatStats,
}

impl AutoCatProgress {
    fn finished(status: &str, message: &str, stats: AutoCatStats) -> Self {
        AutoCatProgress {
            phase: "finished".into(),
            status: status.into(),
            message: message.into(),
            stats,
        }
    }
}

/// 期望分类路径：父目录取前 `depth` 层（根下文件 → None；depth=0 → None）
pub fn desired_path(rel_path: &str, depth: usize) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let parent = rel_path.rsplit_once('/')?.0;
    let segs: Vec<&str> = parent
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect();
    if segs.is_empty() {
        return None;
    }
    let take = depth.min(segs.len());
    Some(segs[..take].join("/"))
}

/// 跑自动分类：单事务；cancel 在条目边界生效；每 200 条节流 emit。
pub fn run(
    conn: &mut Connection,
    root_id: Option<i64>,
    depth: usize,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(&AutoCatProgress),
) -> Result<AutoCatStats, String> {
    let mut stats = AutoCatStats::default();
    if depth == 0 {
        emit(&AutoCatProgress::finished(
            "done",
            "自动分类已关闭（层级 0）",
            stats.clone(),
        ));
        return Ok(stats);
    }

    let tx = conn.transaction().map_err(|e| format!("开事务失败: {e}"))?;
    let mut sql = "SELECT id, rel_path FROM items WHERE missing = 0".to_string();
    let mut args: Vec<i64> = Vec::new();
    if let Some(rid) = root_id {
        sql.push_str(" AND root_id = ?");
        args.push(rid);
    }
    sql.push_str(" ORDER BY id");
    let mut stmt = tx.prepare(&sql).map_err(|e| format!("查询条目失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| format!("查询条目失败: {e}"))?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取条目失败: {e}"))?);
    }
    drop(stmt);

    let total = items.len();
    let mut cancelled = false;
    for (i, (item_id, rel_path)) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        if i % 200 == 0 {
            emit(&AutoCatProgress {
                phase: "queue".into(),
                status: "running".into(),
                message: format!("{}/{total}", i + 1),
                stats: stats.clone(),
            });
        }
        stats.items += 1;

        let desired: Vec<i64> = match desired_path(rel_path, depth) {
            None => Vec::new(),
            Some(p) => vec![categories::ensure_path(&tx, &p)?],
        };

        let existing: Vec<(i64, String)> = {
            let mut s = tx
                .prepare("SELECT category_id, source FROM item_categories WHERE item_id = ?1")
                .map_err(|e| format!("读取关联失败: {e}"))?;
            let rs = s
                .query_map(params![item_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| format!("读取关联失败: {e}"))?;
            let mut v = Vec::new();
            for row in rs {
                v.push(row.map_err(|e| format!("读取关联失败: {e}"))?);
            }
            v
        };
        for (cid, src) in existing {
            if src == "auto" && !desired.contains(&cid) {
                tx.execute(
                    "DELETE FROM item_categories WHERE item_id = ?1 AND category_id = ?2
                     AND source = 'auto'",
                    params![item_id, cid],
                )
                .map_err(|e| format!("移除过期自动关联失败: {e}"))?;
                stats.unlinked += 1;
            }
        }
        for &cid in &desired {
            let n = tx
                .execute(
                    "INSERT INTO item_categories (item_id, category_id, source)
                     VALUES (?1, ?2, 'auto')
                     ON CONFLICT(item_id, category_id) DO NOTHING",
                    params![item_id, cid],
                )
                .map_err(|e| format!("自动归类失败: {e}"))?;
            if n > 0 {
                stats.linked += 1;
            }
        }
    }

    tx.commit().map_err(|e| format!("提交失败: {e}"))?;

    let (status, message) = if cancelled {
        ("cancelled", "已取消")
    } else {
        ("done", "完成")
    };
    emit(&AutoCatProgress::finished(status, message, stats.clone()));
    Ok(stats)
}
