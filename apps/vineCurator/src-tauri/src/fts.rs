//! FTS5 显式同步（无 trigger）：Rust 侧在写入 items / 扩展表 / 标签后同步索引。
//! 迁移 `0003_fts.sql` 建 `fts_items(item_id UNINDEXED, title, body, tags_text)`。

use rusqlite::{params, Connection};

/// 单条 / 全量同步的行构造（items + 扩展表文本 + 标签拼接）。
/// `where_clause` 为 `WHERE i.id = ?1` 或空（全量）。
fn sync_select(where_clause: &str) -> String {
    format!(
        "INSERT INTO fts_items (rowid, item_id, title, body, tags_text)
         SELECT i.id, i.id,
                i.file_name,
                TRIM(
                  i.rel_path || ' ' || COALESCE(CAST(i.year AS TEXT), '') || ' ' || COALESCE(
                    CASE i.type
                      WHEN 'music' THEN (SELECT TRIM(COALESCE(m.artist,'') || ' ' || COALESCE(m.albumartist,'') || ' ' || COALESCE(m.album,'') || ' ' || COALESCE(m.format,''))
                                         FROM music_tracks m WHERE m.item_id = i.id)
                      WHEN 'video' THEN (SELECT TRIM(COALESCE(v.series,'') || ' ' || COALESCE(v.resolution,'') || ' ' || COALESCE(v.codec,''))
                                         FROM video_items v WHERE v.item_id = i.id)
                      WHEN 'book'  THEN (SELECT TRIM(COALESCE(b.author,'') || ' ' || COALESCE(b.series,'') || ' ' || COALESCE(b.volume,'') || ' ' || COALESCE(b.publisher,'') || ' ' || COALESCE(b.isbn,'') || ' ' || COALESCE(b.ocr_text,''))
                                         FROM book_items b WHERE b.item_id = i.id)
                      WHEN 'comic' THEN (SELECT TRIM(COALESCE(c.series,'') || ' ' || COALESCE(c.volume,''))
                                         FROM comic_items c WHERE c.item_id = i.id)
                    END, '')),
                COALESCE((SELECT group_concat(t.name, ' ')
                          FROM item_tags it JOIN tags t ON t.id = it.tag_id
                          WHERE it.item_id = i.id), '')
         FROM items i {where_clause}"
    )
}

/// 同步单条（按 rowid 删除重建；条目已删则净效果为移除索引）。
pub fn sync_item(conn: &Connection, item_id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM fts_items WHERE rowid = ?1", [item_id])
        .map_err(|e| format!("清除 FTS 行失败 id={item_id}: {e}"))?;
    conn.execute(&sync_select("WHERE i.id = ?1"), [item_id])
        .map_err(|e| format!("写入 FTS 行失败 id={item_id}: {e}"))?;
    Ok(())
}

/// 同步一批条目（扫描 flush / 元数据批量导入后调用）。
pub fn sync_items(conn: &Connection, ids: &[i64]) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut n = 0usize;
    for &id in ids {
        sync_item(conn, id)?;
        n += 1;
    }
    Ok(n)
}

/// 全量重建（启动一致性检查 / 手动修复）。
pub fn rebuild_all(conn: &mut Connection) -> Result<usize, String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("重建 FTS 事务失败: {e}"))?;
    let inner = (|| -> Result<usize, String> {
        tx.execute("DELETE FROM fts_items", [])
            .map_err(|e| format!("清空 FTS 失败: {e}"))?;
        tx.execute(&sync_select(""), [])
            .map_err(|e| format!("重建 FTS 失败: {e}"))?;
        let n: i64 = tx
            .query_row("SELECT COUNT(*) FROM fts_items", [], |r| r.get(0))
            .map_err(|e| format!("统计 FTS 失败: {e}"))?;
        Ok(n as usize)
    })();
    match inner {
        Ok(n) => {
            tx.commit().map_err(|e| format!("提交 FTS 重建失败: {e}"))?;
            Ok(n)
        }
        Err(e) => Err(e),
    }
}

/// 清理孤儿行（items 已被删除但 FTS 行残留）。
pub fn prune_orphans(conn: &Connection) -> Result<usize, String> {
    let n = conn
        .execute(
            "DELETE FROM fts_items WHERE rowid NOT IN (SELECT id FROM items)",
            [],
        )
        .map_err(|e| format!("清理 FTS 孤儿行失败: {e}"))?;
    Ok(n)
}

/// 启动一致性检查：条目数 ≠ 索引行数时全量重建。返回是否发生重建。
pub fn ensure_consistent(conn: &mut Connection) -> Result<bool, String> {
    let items: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .map_err(|e| format!("统计 items 失败: {e}"))?;
    let fts: i64 = conn
        .query_row("SELECT COUNT(*) FROM fts_items", [], |r| r.get(0))
        .map_err(|e| format!("统计 fts_items 失败: {e}"))?;
    if items == fts {
        return Ok(false);
    }
    rebuild_all(conn)?;
    Ok(true)
}

/// 用户查询 → 安全 FTS5 MATCH 表达式：每个词加引号（`"` 转义）+ 前缀 `*`，
/// 操作符（AND/OR/NEAR/*）全部成为字面量，绝不注入语法。无有效词返回空串。
pub fn build_match(query: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for tok in query.split_whitespace() {
        let cleaned = tok.replace('"', "");
        if cleaned.is_empty() {
            continue;
        }
        parts.push(format!("\"{cleaned}\"*"));
    }
    parts.join(" ")
}

/// FTS 检索（BM25 排序）。查询为空 / 无有效词返回空 vec。
pub fn search_ids(conn: &Connection, query: &str, limit: i64) -> Result<Vec<i64>, String> {
    let m = build_match(query);
    if m.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT item_id FROM fts_items WHERE fts_items MATCH ?1 ORDER BY rank LIMIT ?2")
        .map_err(|e| format!("准备 FTS 搜索失败: {e}"))?;
    let rows = stmt
        .query_map(params![m, limit], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("FTS 搜索失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取 FTS 结果失败: {e}"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_match_quotes_and_prefixes() {
        assert_eq!(build_match("kind of blue"), "\"kind\"* \"of\"* \"blue\"*");
        assert_eq!(build_match("  dune  "), "\"dune\"*");
    }

    #[test]
    fn build_match_strips_quotes_and_skips_empty() {
        assert_eq!(build_match("\"OR\""), "\"OR\"*");
        assert_eq!(build_match("\"\" \" "), "");
        assert_eq!(build_match(""), "");
    }
}
