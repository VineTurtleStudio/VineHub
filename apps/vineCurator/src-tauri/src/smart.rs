//! 智能集合纯逻辑（零 Tauri）：`query_json` → SQL。
//! 安全不变量：字段/排序列一律走静态白名单映射，**用户值全部占位符绑定**；
//! 不在白名单的字段直接报错（`smart_test` 注入锁死）。

use crate::browse::{self, ItemRow};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

/// 过滤字段白名单（README §6 示例）
pub const FIELD_WHITELIST: &[&str] = &[
    "type",
    "status",
    "rating",
    "year",
    "language",
    "size",
    "mtime",
    "tag",
    "category",
    "has_cover",
    "hash_state",
    "ocr_status",
];

const TEXT_FIELDS: &[&str] = &["type", "status", "language", "hash_state"];
const NUM_FIELDS: &[&str] = &["rating", "year", "size", "mtime", "has_cover"];
const REL_FIELDS: &[&str] = &["tag", "category", "ocr_status"];

/// 排序字段白名单 → 静态列（tag/category/ocr_status 不可排序）
fn sort_column(field: &str) -> Result<&'static str, String> {
    match field {
        "type" => Ok("i.type"),
        "status" => Ok("i.status"),
        "rating" => Ok("i.rating"),
        "year" => Ok("i.year"),
        "language" => Ok("i.language"),
        "size" => Ok("i.size"),
        "mtime" => Ok("i.mtime"),
        "has_cover" => Ok("i.has_cover"),
        "hash_state" => Ok("i.hash_state"),
        "file_name" | "name" => Ok("i.file_name"),
        "rel_path" => Ok("i.rel_path"),
        _ => Err(format!("排序字段不在白名单: {field}")),
    }
}

fn text_column(field: &str) -> Result<&'static str, String> {
    match field {
        "type" => Ok("i.type"),
        "status" => Ok("i.status"),
        "language" => Ok("i.language"),
        "hash_state" => Ok("i.hash_state"),
        _ => Err(format!("字段不在白名单: {field}")),
    }
}

fn num_column(field: &str) -> Result<&'static str, String> {
    match field {
        "rating" => Ok("i.rating"),
        "year" => Ok("i.year"),
        "size" => Ok("i.size"),
        "mtime" => Ok("i.mtime"),
        "has_cover" => Ok("i.has_cover"),
        _ => Err(format!("字段不在白名单: {field}")),
    }
}

/// JSON 标量 → SQLite 绑定值（数组/对象由调用方按 op 展开，不走这里）
fn js_to_sql(v: &Value) -> Result<SqlValue, String> {
    match v {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(b) => Ok(SqlValue::Integer(*b as i64)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(SqlValue::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(SqlValue::Real(f))
            } else {
                Err(format!("数值无法绑定: {n}"))
            }
        }
        Value::String(s) => Ok(SqlValue::Text(s.to_string())),
        _ => Err("value 必须是标量（字符串/数字/布尔）".into()),
    }
}

/// EXISTS 子查询：条目带某标签（精确名或同义词均命中）
const TAG_EXISTS: &str = "EXISTS (SELECT 1 FROM item_tags it JOIN tags t ON t.id = it.tag_id
         WHERE it.item_id = i.id AND (t.name = ?
           OR EXISTS (SELECT 1 FROM tag_aliases ta WHERE ta.alias = ? AND ta.tag_id = t.id)))";

const OCR_EXISTS: &str =
    "EXISTS (SELECT 1 FROM book_items b WHERE b.item_id = i.id AND b.ocr_status = ?)";

/// 编译单个过滤节点 → (SQL 片段, 绑定参数)
fn compile(node: &Value) -> Result<(String, Vec<SqlValue>), String> {
    let obj = node
        .as_object()
        .ok_or_else(|| "过滤节点必须是对象".to_string())?;

    if let Some(children) = obj.get("all") {
        let arr = children
            .as_array()
            .ok_or_else(|| "all 必须是数组".to_string())?;
        if arr.is_empty() {
            return Ok(("1=1".into(), Vec::new()));
        }
        let mut parts = Vec::with_capacity(arr.len());
        let mut args = Vec::new();
        for child in arr {
            let (sql, a) = compile(child)?;
            parts.push(format!("({sql})"));
            args.extend(a);
        }
        return Ok((parts.join(" AND "), args));
    }
    if let Some(children) = obj.get("any") {
        let arr = children
            .as_array()
            .ok_or_else(|| "any 必须是数组".to_string())?;
        if arr.is_empty() {
            return Ok(("0=1".into(), Vec::new()));
        }
        let mut parts = Vec::with_capacity(arr.len());
        let mut args = Vec::new();
        for child in arr {
            let (sql, a) = compile(child)?;
            parts.push(format!("({sql})"));
            args.extend(a);
        }
        return Ok((parts.join(" OR "), args));
    }
    if let Some(inner) = obj.get("not") {
        let (sql, args) = compile(inner)?;
        return Ok((format!("(NOT {sql})"), args));
    }

    // 叶子条件
    let field = obj
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| "条件缺少 field".to_string())?;
    if !FIELD_WHITELIST.contains(&field) {
        return Err(format!("字段不在白名单: {field}"));
    }
    let op = obj.get("op").and_then(Value::as_str).unwrap_or("eq");
    let value = obj
        .get("value")
        .ok_or_else(|| "条件缺少 value".to_string())?;

    compile_condition(field, op, value)
}

fn compile_condition(
    field: &str,
    op: &str,
    value: &Value,
) -> Result<(String, Vec<SqlValue>), String> {
    if REL_FIELDS.contains(&field) {
        return compile_relation(field, op, value);
    }
    let numeric = NUM_FIELDS.contains(&field);
    if !numeric && !TEXT_FIELDS.contains(&field) {
        return Err(format!("字段不在白名单: {field}"));
    }
    // ocr_status 在 REL_FIELDS 已分流；这里只可能是列字段

    match op {
        "eq" | "neq" => {
            let sql = if numeric {
                let col = num_column(field)?;
                format!("{col} {cmp} ?", cmp = if op == "eq" { "=" } else { "<>" })
            } else {
                let col = text_column(field)?;
                format!("{col} {cmp} ?", cmp = if op == "eq" { "=" } else { "<>" })
            };
            Ok((sql, vec![js_to_sql(value)?]))
        }
        "gt" | "gte" | "lt" | "lte" => {
            if !numeric {
                return Err(format!("{op} 仅支持数值字段: {field}"));
            }
            let col = num_column(field)?;
            let cmp = match op {
                "gt" => ">",
                "gte" => ">=",
                "lt" => "<",
                _ => "<=",
            };
            Ok((format!("{col} {cmp} ?"), vec![js_to_sql(value)?]))
        }
        "contains" => {
            if numeric {
                return Err(format!("contains 仅支持文本字段: {field}"));
            }
            let col = text_column(field)?;
            let s = value
                .as_str()
                .ok_or_else(|| "contains 的 value 必须是字符串".to_string())?;
            Ok((
                format!("{col} LIKE ? ESCAPE '\\'"),
                vec![SqlValue::Text(browse::like_pattern(s))],
            ))
        }
        "in" => {
            let arr = value
                .as_array()
                .ok_or_else(|| "in 的 value 必须是数组".to_string())?;
            if arr.is_empty() {
                return Ok(("0=1".into(), Vec::new()));
            }
            let col = if numeric {
                num_column(field)?
            } else {
                text_column(field)?
            };
            let placeholders = vec!["?"; arr.len()].join(", ");
            let args = arr.iter().map(js_to_sql).collect::<Result<Vec<_>, _>>()?;
            Ok((format!("{col} IN ({placeholders})"), args))
        }
        _ => Err(format!("不支持的操作符: {op}")),
    }
}

fn compile_relation(
    field: &str,
    op: &str,
    value: &Value,
) -> Result<(String, Vec<SqlValue>), String> {
    match field {
        "tag" => {
            if op != "eq" && op != "neq" {
                return Err(format!("tag 仅支持 eq/neq，收到: {op}"));
            }
            let s = value
                .as_str()
                .ok_or_else(|| "tag 的 value 必须是字符串".to_string())?;
            let sql = if op == "eq" {
                TAG_EXISTS.to_string()
            } else {
                format!("NOT {TAG_EXISTS}")
            };
            let t = SqlValue::Text(s.to_string());
            Ok((sql, vec![t.clone(), t]))
        }
        "category" => {
            if op != "eq" && op != "neq" {
                return Err(format!("category 仅支持 eq/neq，收到: {op}"));
            }
            let not = if op == "eq" { "" } else { "NOT " };
            match value {
                Value::Number(n) => {
                    let id = n
                        .as_i64()
                        .ok_or_else(|| "category id 必须是整数".to_string())?;
                    Ok((
                        format!(
                            "{not}i.id IN (SELECT item_id FROM item_categories WHERE category_id = ?)"
                        ),
                        vec![SqlValue::Integer(id)],
                    ))
                }
                Value::String(s) => Ok((
                    format!(
                        "{not}i.id IN (SELECT ic.item_id FROM item_categories ic
                             JOIN categories c ON c.id = ic.category_id WHERE c.path = ?)"
                    ),
                    vec![SqlValue::Text(s.to_string())],
                )),
                _ => Err("category 的 value 必须是 id(数字) 或 path(字符串)".into()),
            }
        }
        "ocr_status" => {
            if op != "eq" && op != "neq" {
                return Err(format!("ocr_status 仅支持 eq/neq，收到: {op}"));
            }
            let s = value
                .as_str()
                .ok_or_else(|| "ocr_status 的 value 必须是字符串".to_string())?;
            let sql = if op == "eq" {
                OCR_EXISTS.to_string()
            } else {
                format!("NOT {OCR_EXISTS}")
            };
            Ok((sql, vec![SqlValue::Text(s.to_string())]))
        }
        _ => Err(format!("字段不在白名单: {field}")),
    }
}

/// 编译完整 query_json → (where_sql, 绑定参数, order_sql, limit)
/// `filter` 缺省 = 全量；`sort` 缺省 = file_name ASC；limit 钳制 1..=1000
pub fn compile_query(query_json: &str) -> Result<(String, Vec<SqlValue>, String, i64), String> {
    let v: Value =
        serde_json::from_str(query_json).map_err(|e| format!("query_json 解析失败: {e}"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| "query_json 必须是对象".to_string())?;

    let (where_sql, args) = if let Some(filter) = obj.get("filter") {
        compile(filter)?
    } else {
        ("1=1".to_string(), Vec::new())
    };

    let mut order = String::from("i.file_name ASC");
    if let Some(sort) = obj.get("sort") {
        let arr = sort
            .as_array()
            .ok_or_else(|| "sort 必须是数组".to_string())?;
        if !arr.is_empty() {
            let mut parts = Vec::with_capacity(arr.len());
            for s in arr {
                let o = s
                    .as_object()
                    .ok_or_else(|| "sort 项必须是对象".to_string())?;
                let field = o
                    .get("field")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "sort 项缺少 field".to_string())?;
                let col = sort_column(field)?;
                let dir = match o.get("dir").and_then(Value::as_str).unwrap_or("asc") {
                    "asc" => "ASC",
                    "desc" => "DESC",
                    other => return Err(format!("排序方向非法: {other}")),
                };
                parts.push(format!("{col} {dir}"));
            }
            parts.push("i.id ASC".into());
            order = parts.join(", ");
        }
    }

    let limit = obj
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(200)
        .clamp(1, 1000);

    Ok((where_sql, args, order, limit))
}

/// 预览：返回 (总命中数, 分页条目)
pub fn preview(
    conn: &Connection,
    query_json: &str,
    offset: Option<i64>,
) -> Result<(i64, Vec<ItemRow>), String> {
    let (where_sql, args, order, limit) = compile_query(query_json)?;
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM items i WHERE {where_sql}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )
        .map_err(|e| format!("统计智能集合失败: {e}"))?;
    let offset = offset.unwrap_or(0).max(0);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM items i WHERE {where_sql} ORDER BY {order} LIMIT {limit} OFFSET {offset}",
            browse::item_cols()
        ))
        .map_err(|e| format!("查询智能集合失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), browse::map_item)
        .map_err(|e| format!("查询智能集合失败: {e}"))?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取条目失败: {e}"))?);
    }
    Ok((total, items))
}

// ---------------------------------------------------------------- 集合 CRUD

#[derive(Debug, Clone, Serialize)]
pub struct CollectionRow {
    pub id: i64,
    pub name: String,
    pub query_json: String,
    pub sort_order: i64,
}

pub fn list_collections(conn: &Connection) -> Result<Vec<CollectionRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, query_json, sort_order FROM smart_collections
             ORDER BY sort_order, name",
        )
        .map_err(|e| format!("读取集合失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(CollectionRow {
                id: r.get(0)?,
                name: r.get(1)?,
                query_json: r.get(2)?,
                sort_order: r.get(3)?,
            })
        })
        .map_err(|e| format!("读取集合失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取集合失败: {e}"))?);
    }
    Ok(out)
}

/// 保存（id 缺省 = 新建）：先编译校验 query_json，再落库；名称全局唯一
pub fn save_collection(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    query_json: &str,
    sort_order: i64,
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("集合名不能为空".into());
    }
    compile_query(query_json)?; // 保存前即校验白名单
    match id {
        Some(id) => {
            let n = conn
                .execute(
                    "UPDATE smart_collections SET name = ?1, query_json = ?2,
                     sort_order = ?3, updated_at = datetime('now') WHERE id = ?4",
                    rusqlite::params![name, query_json, sort_order, id],
                )
                .map_err(|e| format!("保存集合失败: {e}"))?;
            if n == 0 {
                return Err(format!("集合不存在: {id}"));
            }
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO smart_collections (name, query_json, sort_order)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![name, query_json, sort_order],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    "已存在同名集合".to_string()
                } else {
                    format!("保存集合失败: {e}")
                }
            })?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// 三个示例集合（README 验收）：仅首次播种一次（settings 标记 `examples_seeded`），
/// 幂等；`INSERT OR IGNORE` 遇同名跳过，用户删光后不会被重新播种。
pub fn seed_example_collections(conn: &Connection) -> Result<(), String> {
    let seeded: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'examples_seeded'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("读取播种标记失败: {e}"))?;
    if seeded.as_deref() == Some("1") {
        return Ok(());
    }
    let examples: [(&str, &str); 3] = [
        (
            "未读且=科幻",
            r#"{"filter":{"all":[{"field":"status","op":"in","value":["inbox","unwatched","reading"]},{"field":"tag","op":"eq","value":"科幻"}]},"sort":[{"field":"mtime","dir":"desc"}],"limit":200}"#,
        ),
        (
            "4K 未看",
            r#"{"filter":{"all":[{"field":"type","op":"eq","value":"video"},{"field":"status","op":"in","value":["inbox","unwatched"]},{"field":"size","op":"gt","value":8589934592}]},"sort":[{"field":"size","dir":"desc"}],"limit":200}"#,
        ),
        (
            "待 OCR",
            r#"{"filter":{"all":[{"field":"type","op":"eq","value":"book"},{"field":"ocr_status","op":"eq","value":"none"}]},"sort":[{"field":"mtime","dir":"asc"}],"limit":200}"#,
        ),
    ];
    for (i, (name, query)) in examples.iter().enumerate() {
        compile_query(query).map_err(|e| format!("示例集合「{name}」非法: {e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO smart_collections (name, query_json, sort_order)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![name, query, i as i64],
        )
        .map_err(|e| format!("播种示例集合失败: {e}"))?;
    }
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('examples_seeded', '1')
         ON CONFLICT(key) DO UPDATE SET value = '1'",
        [],
    )
    .map_err(|e| format!("写入播种标记失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_rejects_unknown_field() {
        assert!(
            compile_query(r#"{"filter":{"field":"drop table","op":"eq","value":"x"}}"#).is_err()
        );
        assert!(compile_query(r#"{"filter":{"field":"root_id","op":"eq","value":1}}"#).is_err());
        assert!(compile_query(r#"{"sort":[{"field":"metadata","dir":"asc"}]}"#).is_err());
    }

    #[test]
    fn values_are_bound_not_interpolated() {
        let (where_sql, _, _, _) = compile_query(
            r#"{"filter":{"field":"status","op":"eq","value":"x'; DROP TABLE items;--"}}"#,
        )
        .expect("compile");
        assert!(where_sql.contains('?'), "值必须走占位符: {where_sql}");
        assert!(
            !where_sql.contains("DROP"),
            "值绝不进 SQL 文本: {where_sql}"
        );
    }

    #[test]
    fn empty_filters_and_limit_clamp() {
        let (_, _, _, limit) = compile_query("{}").expect("empty");
        assert_eq!(limit, 200);
        let (_, _, _, limit) = compile_query(r#"{"limit":99999}"#).expect("big");
        assert_eq!(limit, 1000);
        let (w, _, _, _) = compile_query(r#"{"filter":{"all":[]}}"#).expect("all empty");
        assert_eq!(w, "1=1");
        let (w, _, _, _) = compile_query(r#"{"filter":{"any":[]}}"#).expect("any empty");
        assert_eq!(w, "0=1");
        assert!(compile_query(r#"{"filter":{"field":"type","op":"wat","value":"x"}}"#).is_err());
        assert!(
            compile_query(r#"{"filter":{"field":"year","op":"contains","value":"x"}}"#).is_err()
        );
        assert!(compile_query(r#"{"filter":{"field":"type","op":"in","value":[]}}"#).is_ok());
    }
}
