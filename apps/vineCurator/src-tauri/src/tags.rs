//! 标签纯逻辑（零 Tauri）：受控词表 + 同义词别名 + 批量打标 + 合并。
//! 写入后由调用方触发 `fts::sync_item`（tags_text 列）。

use rusqlite::{params, Connection, OptionalExtension};

const MAX_NAME: usize = 60;

/// 标签名归一化：trim、非空、不含逗号（批量分隔）、长度受限
pub fn validate_tag_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("标签名不能为空".into());
    }
    if n.contains(',') {
        return Err("标签名不能包含逗号".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("标签名过长（最多 {MAX_NAME} 字符）"));
    }
    Ok(n.to_string())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TagRow {
    pub id: i64,
    pub name: String,
    pub item_count: i64,
    pub aliases: Vec<String>,
}

/// 解析标签名 → id：先查精确名，再查同义词（tag_aliases），最后按需创建
fn resolve_tag(conn: &Connection, name: &str) -> Result<i64, String> {
    let direct: Option<i64> = conn
        .query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0))
        .optional()
        .map_err(|e| format!("查询标签失败: {e}"))?;
    if let Some(id) = direct {
        return Ok(id);
    }
    let alias: Option<i64> = conn
        .query_row(
            "SELECT tag_id FROM tag_aliases WHERE alias = ?1",
            [name],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("查询同义词失败: {e}"))?;
    if let Some(id) = alias {
        return Ok(id);
    }
    conn.execute("INSERT INTO tags (name) VALUES (?1)", [name])
        .map_err(|e| format!("创建标签失败: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// 标签列表（带条目计数与同义词）
pub fn list_tags(conn: &Connection) -> Result<Vec<TagRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.name,
                    (SELECT COUNT(*) FROM item_tags it WHERE it.tag_id = t.id)
             FROM tags t ORDER BY t.name",
        )
        .map_err(|e| format!("读取标签失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("读取标签失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, item_count) = row.map_err(|e| format!("读取标签失败: {e}"))?;
        let aliases = aliases_of(conn, id)?;
        out.push(TagRow {
            id,
            name,
            item_count,
            aliases,
        });
    }
    Ok(out)
}

fn aliases_of(conn: &Connection, tag_id: i64) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT alias FROM tag_aliases WHERE tag_id = ?1 ORDER BY alias")
        .map_err(|e| format!("读取同义词失败: {e}"))?;
    let rows = stmt
        .query_map([tag_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("读取同义词失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取同义词失败: {e}"))?);
    }
    Ok(out)
}

/// 创建标签（幂等：已存在返回现 id）；别名冲突会合并到现有标签
pub fn create_tag(conn: &Connection, name: &str) -> Result<i64, String> {
    let name = validate_tag_name(name)?;
    resolve_tag(conn, &name)
}

/// 改名：同义词保留指向；被占用则报错
pub fn rename_tag(conn: &Connection, tag_id: i64, new_name: &str) -> Result<(), String> {
    let name = validate_tag_name(new_name)?;
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM tags WHERE name = ?1", [&name], |r| r.get(0))
        .optional()
        .map_err(|e| format!("查询标签失败: {e}"))?;
    if let Some(other) = exists {
        if other != tag_id {
            return Err("已存在同名标签".into());
        }
        return Ok(());
    }
    let n = conn
        .execute(
            "UPDATE tags SET name = ?1 WHERE id = ?2",
            params![name, tag_id],
        )
        .map_err(|e| format!("改名失败: {e}"))?;
    if n == 0 {
        return Err("标签不存在".into());
    }
    Ok(())
}

/// 注册同义词：alias → tag_id（alias 若是别的标签名则报错）
pub fn add_alias(conn: &Connection, alias: &str, tag_id: i64) -> Result<(), String> {
    let alias = validate_tag_name(alias)?;
    let owner: Option<i64> = conn
        .query_row("SELECT id FROM tags WHERE name = ?1", [&alias], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| format!("查询标签失败: {e}"))?;
    if let Some(o) = owner {
        if o == tag_id {
            return Ok(()); // 自身名做别名无意义但无害
        }
        return Err("该名称已是另一个标签".into());
    }
    conn.execute(
        "INSERT INTO tag_aliases (alias, tag_id) VALUES (?1, ?2)
         ON CONFLICT(alias) DO UPDATE SET tag_id = excluded.tag_id",
        params![alias, tag_id],
    )
    .map_err(|e| format!("写入同义词失败: {e}"))?;
    Ok(())
}

/// 合并：src 并入 dst（item_tags 去重迁移、别名迁移），删除 src。返回受影响条目数。
pub fn merge_tags(conn: &mut Connection, src_id: i64, dst_id: i64) -> Result<usize, String> {
    if src_id == dst_id {
        return Err("不能合并到自身".into());
    }
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<usize, String> {
        let (src_name, dst_name) = tx
            .query_row(
                "SELECT (SELECT name FROM tags WHERE id = ?1), (SELECT name FROM tags WHERE id = ?2)",
                params![src_id, dst_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(|_| format!("标签不存在: {src_id} / {dst_id}"))?;
        if src_name.is_empty() || dst_name.is_empty() {
            return Err("标签不存在".into());
        }
        let moved = tx
            .execute(
                "INSERT INTO item_tags (item_id, tag_id, source, created_at)
                 SELECT item_id, ?2, source, created_at FROM item_tags WHERE tag_id = ?1
                 ON CONFLICT(item_id, tag_id) DO NOTHING",
                params![src_id, dst_id],
            )
            .map_err(|e| format!("迁移打标失败: {e}"))?;
        // 别名迁移：alias 是主键，改 tag_id 不触碰主键，单条 UPDATE 即可
        tx.execute(
            "UPDATE tag_aliases SET tag_id = ?1 WHERE tag_id = ?2",
            params![dst_id, src_id],
        )
        .map_err(|e| format!("迁移同义词失败: {e}"))?;
        tx.execute("DELETE FROM tags WHERE id = ?1", [src_id])
            .map_err(|e| format!("删除源标签失败: {e}"))?;
        Ok(moved)
    })();
    match inner {
        Ok(n) => {
            tx.commit().map_err(|e| format!("提交失败: {e}"))?;
            Ok(n)
        }
        Err(e) => Err(e),
    }
}

/// 批量打标：返回本批实际变更的 (item_id, tag_id) 对
pub struct TagAssignment {
    pub item_id: i64,
    pub tag_id: i64,
}

/// 给条目设置标签集合（增量：只新增给定标签，不删已有）。
/// 返回写入的 (item_id, tag_id)，调用方据此同步 FTS。
/// 先整体校验（名称 + 条目存在），再解析创建，避免失败调用留下孤儿标签。
pub fn add_tags_to_items(
    conn: &mut Connection,
    item_ids: &[i64],
    names: &[String],
) -> Result<Vec<TagAssignment>, String> {
    if item_ids.is_empty() || names.is_empty() {
        return Ok(Vec::new());
    }
    let mut validated = Vec::with_capacity(names.len());
    for n in names {
        validated.push(validate_tag_name(n)?);
    }
    for &item_id in item_ids {
        let exists: i64 = conn
            .query_row("SELECT 1 FROM items WHERE id = ?1", [item_id], |r| {
                r.get::<_, i64>(0)
            })
            .optional()
            .map_err(|e| format!("查询条目失败: {e}"))?
            .unwrap_or(0);
        if exists == 0 {
            return Err(format!("条目不存在: {item_id}"));
        }
    }
    let mut resolved = Vec::with_capacity(validated.len());
    for name in &validated {
        resolved.push(resolve_tag(conn, name)?);
    }
    let mut out = Vec::new();
    for &item_id in item_ids {
        for &tag_id in &resolved {
            let n = conn
                .execute(
                    "INSERT INTO item_tags (item_id, tag_id, source) VALUES (?1, ?2, 'manual')
                     ON CONFLICT(item_id, tag_id) DO NOTHING",
                    params![item_id, tag_id],
                )
                .map_err(|e| format!("打标失败: {e}"))?;
            if n > 0 {
                out.push(TagAssignment { item_id, tag_id });
            }
        }
    }
    Ok(out)
}

/// 移除条目标签（不动 tags 词表）
pub fn remove_tags_from_items(
    conn: &Connection,
    item_ids: &[i64],
    tag_ids: &[i64],
) -> Result<Vec<i64>, String> {
    let mut touched = Vec::new();
    for &item_id in item_ids {
        for &tag_id in tag_ids {
            let n = conn
                .execute(
                    "DELETE FROM item_tags WHERE item_id = ?1 AND tag_id = ?2",
                    params![item_id, tag_id],
                )
                .map_err(|e| format!("移除标签失败: {e}"))?;
            if n > 0 {
                touched.push(item_id);
            }
        }
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_tag_name_rules() {
        assert!(validate_tag_name("").is_err());
        assert!(validate_tag_name(" a,b ").is_err());
        assert_eq!(validate_tag_name("  4K  ").unwrap(), "4K");
        assert!(validate_tag_name(&"x".repeat(61)).is_err());
    }
}
