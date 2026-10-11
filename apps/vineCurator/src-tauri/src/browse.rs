use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const ITEM_COLS: &str = "i.id, i.root_id, i.rel_path, i.file_name, i.ext, i.type, i.size, i.mtime, i.status, i.rating, i.year, i.missing, i.hash_state, i.meta_state";

/// 列清单（供 smart.rs 复用）
pub(crate) fn item_cols() -> &'static str {
    ITEM_COLS
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemRow {
    pub id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub file_name: String,
    pub ext: String,
    pub type_name: String,
    pub size: i64,
    pub mtime: i64,
    pub status: String,
    pub rating: i64,
    pub year: Option<i64>,
    pub missing: bool,
    pub hash_state: String,
    pub meta_state: String,
}

pub(crate) fn map_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        id: r.get(0)?,
        root_id: r.get(1)?,
        rel_path: r.get(2)?,
        file_name: r.get(3)?,
        ext: r.get(4)?,
        type_name: r.get(5)?,
        size: r.get(6)?,
        mtime: r.get(7)?,
        status: r.get(8)?,
        rating: r.get(9)?,
        year: r.get(10)?,
        missing: r.get(11)?,
        hash_state: r.get(12)?,
        meta_state: r.get(13)?,
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemFilter {
    pub root_id: Option<i64>,
    pub type_name: Option<String>,
    pub status: Option<String>,
    pub category_id: Option<i64>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub sort: Option<String>,
    pub desc: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ItemPage {
    pub total: i64,
    pub items: Vec<ItemRow>,
}

/// 排序字段白名单：只映射到静态列名，用户输入绝不拼进 SQL
fn sort_column(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("") {
        "file_name" | "name" => "i.file_name",
        "rel_path" => "i.rel_path",
        "size" => "i.size",
        "mtime" => "i.mtime",
        "year" => "i.year",
        "rating" => "i.rating",
        "type_name" | "type" => "i.type",
        "status" => "i.status",
        _ => "i.file_name",
    }
}

pub fn list_items(conn: &Connection, f: &ItemFilter) -> Result<ItemPage, String> {
    let mut conds: Vec<String> = vec!["1=1".to_string()];
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(rid) = f.root_id {
        conds.push("i.root_id = ?".into());
        args.push(rid.into());
    }
    if let Some(t) = f.type_name.as_deref().filter(|s| !s.is_empty()) {
        conds.push("i.type = ?".into());
        args.push(t.to_string().into());
    }
    if let Some(s) = f.status.as_deref().filter(|s| !s.is_empty()) {
        conds.push("i.status = ?".into());
        args.push(s.to_string().into());
    }
    if let Some(cid) = f.category_id {
        conds.push("i.id IN (SELECT item_id FROM item_categories WHERE category_id = ?)".into());
        args.push(cid.into());
    }
    let where_sql = conds.join(" AND ");

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM items i WHERE {where_sql}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )
        .map_err(|e| format!("统计条目失败: {e}"))?;

    let order = sort_column(f.sort.as_deref());
    let dir = if f.desc.unwrap_or(false) {
        "DESC"
    } else {
        "ASC"
    };
    let limit = f.limit.unwrap_or(200).clamp(1, 1000);
    let offset = f.offset.unwrap_or(0).max(0);
    args.push(limit.into());
    args.push(offset.into());

    let sql = format!(
        "SELECT {ITEM_COLS} FROM items i WHERE {where_sql} ORDER BY {order} {dir}, i.id ASC LIMIT ? OFFSET ?"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("查询条目失败: {e}"))?;
    let rows = stmt
        .query_map(params_from_iter(args.iter()), map_item)
        .map_err(|e| format!("查询条目失败: {e}"))?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取条目失败: {e}"))?);
    }
    Ok(ItemPage { total, items })
}

// ---------------------------------------------------------------- 分类树

#[derive(Debug, Clone, Serialize)]
pub struct CategoryNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub item_count: i64,
    pub children: Vec<CategoryNode>,
}

/// 分类树（物化路径按字典序 → 父先于子），带条目计数
pub fn list_categories(conn: &Connection) -> Result<Vec<CategoryNode>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.parent_id, c.name, c.path,
                    (SELECT COUNT(*) FROM item_categories ic WHERE ic.category_id = c.id)
             FROM categories c ORDER BY c.path, c.sort_order, c.name",
        )
        .map_err(|e| format!("查询分类失败: {e}"))?;
    struct Flat {
        id: i64,
        parent_id: Option<i64>,
        name: String,
        path: String,
        count: i64,
    }
    let rows = stmt
        .query_map([], |r| {
            Ok(Flat {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                path: r.get(3)?,
                count: r.get(4)?,
            })
        })
        .map_err(|e| format!("查询分类失败: {e}"))?;
    let mut flat = Vec::new();
    for row in rows {
        flat.push(row.map_err(|e| format!("读取分类失败: {e}"))?);
    }

    let mut children: HashMap<Option<i64>, Vec<usize>> = HashMap::new();
    for (i, node) in flat.iter().enumerate() {
        children.entry(node.parent_id).or_default().push(i);
    }
    fn build(
        parent: Option<i64>,
        flat: &[Flat],
        children: &mut HashMap<Option<i64>, Vec<usize>>,
    ) -> Vec<CategoryNode> {
        let Some(idxs) = children.remove(&parent) else {
            return Vec::new();
        };
        idxs.into_iter()
            .map(|i| {
                let n = &flat[i];
                CategoryNode {
                    id: n.id,
                    parent_id: n.parent_id,
                    name: n.name.clone(),
                    path: n.path.clone(),
                    item_count: n.count,
                    children: build(Some(n.id), flat, children),
                }
            })
            .collect()
    }
    Ok(build(None, &flat, &mut children))
}

// ---------------------------------------------------------------- 概览

#[derive(Debug, Clone, Serialize)]
pub struct CountEntry {
    pub key: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub total: i64,
    pub missing: i64,
    pub by_type: Vec<CountEntry>,
    pub by_status: Vec<CountEntry>,
}

pub fn overview(conn: &Connection, root_id: Option<i64>) -> Result<Overview, String> {
    let scope = if root_id.is_some() {
        " WHERE root_id = ?"
    } else {
        ""
    };
    let missing_scope = if root_id.is_some() {
        " AND root_id = ?"
    } else {
        ""
    };
    let arg = [root_id];
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM items{scope}"),
            params_from_iter(arg.iter().flatten()),
            |r| r.get(0),
        )
        .map_err(|e| format!("统计失败: {e}"))?;
    let missing: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM items WHERE missing = 1{missing_scope}"),
            params_from_iter(arg.iter().flatten()),
            |r| r.get(0),
        )
        .map_err(|e| format!("统计失败: {e}"))?;

    let group = |col: &str| -> Result<Vec<CountEntry>, String> {
        let sql = format!("SELECT {col}, COUNT(*) FROM items{scope} GROUP BY {col} ORDER BY {col}");
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("统计失败: {e}"))?;
        let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<CountEntry> {
            Ok(CountEntry {
                key: r.get(0)?,
                count: r.get(1)?,
            })
        };
        let rows = if root_id.is_some() {
            stmt.query_map(params![root_id], map)
        } else {
            stmt.query_map([], map)
        }
        .map_err(|e| format!("统计失败: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取统计失败: {e}"))?);
        }
        Ok(out)
    };

    Ok(Overview {
        total,
        missing,
        by_type: group("type")?,
        by_status: group("status")?,
    })
}

// ---------------------------------------------------------------- 详情

#[derive(Debug, Clone, Serialize)]
pub struct ExtField {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagEntry {
    pub id: i64,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemDetail {
    pub item: ItemRow,
    pub root_path: String,
    pub ext: Vec<ExtField>,
    pub tags: Vec<TagEntry>,
    pub category_ids: Vec<i64>,
    pub metadata: String,
}

fn value_to_text(v: rusqlite::types::ValueRef<'_>) -> Option<String> {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        }
        ValueRef::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
    }
}

pub fn get_item(conn: &Connection, item_id: i64) -> Result<ItemDetail, String> {
    let (item, root_path, metadata) = conn
        .query_row(
            &format!(
                "SELECT {ITEM_COLS}, r.path, i.metadata FROM items i
                 JOIN roots r ON r.id = i.root_id WHERE i.id = ?1"
            ),
            [item_id],
            |r| {
                let item = map_item(r)?;
                Ok((item, r.get::<_, String>(14)?, r.get::<_, String>(15)?))
            },
        )
        .map_err(|_| format!("条目不存在: {item_id}"))?;

    let mut ext = Vec::new();
    if let Some(table) = crate::scan::classify::ext_table(&item.type_name) {
        let sql = format!("SELECT * FROM {table} WHERE item_id = ?1");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("读取 {table} 失败: {e}"))?;
        let names: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let row = stmt
            .query_row([item_id], |r| {
                let mut vals = Vec::with_capacity(names.len());
                for i in 0..names.len() {
                    vals.push(value_to_text(r.get_ref(i)?));
                }
                Ok(vals)
            })
            .optional()
            .map_err(|e| format!("读取 {table} 失败: {e}"))?;
        if let Some(vals) = row {
            for (i, v) in vals.into_iter().enumerate() {
                let name = &names[i];
                if name == "item_id" {
                    continue;
                }
                ext.push(ExtField {
                    name: name.clone(),
                    value: v,
                });
            }
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.name, it.source FROM item_tags it
             JOIN tags t ON t.id = it.tag_id WHERE it.item_id = ?1 ORDER BY t.name",
        )
        .map_err(|e| format!("读取标签失败: {e}"))?;
    let rows = stmt
        .query_map([item_id], |r| {
            Ok(TagEntry {
                id: r.get(0)?,
                name: r.get(1)?,
                source: r.get(2)?,
            })
        })
        .map_err(|e| format!("读取标签失败: {e}"))?;
    let mut tags = Vec::new();
    for row in rows {
        tags.push(row.map_err(|e| format!("读取标签失败: {e}"))?);
    }

    let mut stmt = conn
        .prepare("SELECT category_id FROM item_categories WHERE item_id = ?1")
        .map_err(|e| format!("读取分类关联失败: {e}"))?;
    let rows = stmt
        .query_map([item_id], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("读取分类关联失败: {e}"))?;
    let mut category_ids = Vec::new();
    for row in rows {
        category_ids.push(row.map_err(|e| format!("读取分类关联失败: {e}"))?);
    }

    Ok(ItemDetail {
        item,
        root_path,
        ext,
        tags,
        category_ids,
        metadata,
    })
}

// ---------------------------------------------------------------- 搜索

/// LIKE 模式：转义 \ % _（ESCAPE '\'）
pub fn like_pattern(query: &str) -> String {
    format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

pub fn search_items(
    conn: &Connection,
    query: &str,
    limit: Option<i64>,
) -> Result<Vec<ItemRow>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(200).clamp(1, 1000);

    // 1) FTS5（BM25 排序），失败或无结果回落 LIKE 子串
    let ids = crate::fts::search_ids(conn, q, limit).unwrap_or_default();
    if !ids.is_empty() {
        return rows_by_ids(conn, &ids);
    }

    // 2) LIKE 兜底（子串 / FTS 分词未覆盖的短查询）
    let pattern = like_pattern(q);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ITEM_COLS} FROM items i
             WHERE i.file_name LIKE ?1 ESCAPE '\\' OR i.rel_path LIKE ?1 ESCAPE '\\'
             ORDER BY i.file_name LIMIT ?2"
        ))
        .map_err(|e| format!("搜索失败: {e}"))?;
    let rows = stmt
        .query_map(params![pattern, limit], map_item)
        .map_err(|e| format!("搜索失败: {e}"))?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取搜索结果失败: {e}"))?);
    }
    Ok(items)
}

/// 按 id 顺序取行（保持 FTS rank 排序），跳过已删除条目
fn rows_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<ItemRow>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {ITEM_COLS} FROM items i WHERE i.id = ?1"))
        .map_err(|e| format!("读取搜索结果失败: {e}"))?;
    let mut by_id: HashMap<i64, ItemRow> = HashMap::new();
    for &id in ids {
        let row = stmt
            .query_row([id], map_item)
            .optional()
            .map_err(|e| format!("读取搜索结果失败: {e}"))?;
        if let Some(item) = row {
            by_id.insert(id, item);
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}
