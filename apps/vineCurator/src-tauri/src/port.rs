//! 标签/分类 JSON 导入导出（纯逻辑，零 Tauri）：物化路径分类树 + 标签词表/别名的
//! 可迁移快照。导入为增量 upsert：同 path 更新 name/sort_order、缺失祖先按路径补建、
//! 标签按名 upsert、别名冲突跳过；全部参数绑定（恶意路径/标签名不拼接）。
//! 只动词表与分类树，不改 item_tags 关联 → 无需 FTS 重建。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{categories, tags};

pub const PORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortCategory {
    pub path: String,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortTag {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortDoc {
    pub version: u32,
    pub exported_at: String,
    pub categories: Vec<PortCategory>,
    pub tags: Vec<PortTag>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub categories_created: usize,
    pub categories_updated: usize,
    pub tags_created: usize,
    pub aliases_added: usize,
    pub skipped: Vec<String>,
}

pub fn export_doc(conn: &Connection) -> Result<PortDoc, String> {
    let mut stmt = conn
        .prepare("SELECT path, name, sort_order FROM categories ORDER BY path")
        .map_err(|e| format!("读取分类失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PortCategory {
                path: r.get(0)?,
                name: r.get(1)?,
                sort_order: r.get(2)?,
            })
        })
        .map_err(|e| format!("读取分类失败: {e}"))?;
    let mut categories = Vec::new();
    for row in rows {
        categories.push(row.map_err(|e| format!("读取分类失败: {e}"))?);
    }

    let mut tag_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT t.name, COALESCE(a.alias, '') FROM tags t
             LEFT JOIN tag_aliases a ON a.tag_id = t.id
             ORDER BY t.name, a.alias",
        )
        .map_err(|e| format!("读取标签失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("读取标签失败: {e}"))?;
    for row in rows {
        let (tag, alias) = row.map_err(|e| format!("读取标签失败: {e}"))?;
        let entry = tag_map.entry(tag).or_default();
        if !alias.is_empty() {
            entry.push(alias);
        }
    }
    let tags = tag_map
        .into_iter()
        .map(|(name, aliases)| PortTag { name, aliases })
        .collect();

    let exported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();
    Ok(PortDoc {
        version: PORT_VERSION,
        exported_at,
        categories,
        tags,
    })
}

/// 路径拆段：trim、拒绝空/前导尾随 `/`/空段（"a//b"、"/a"、"a/" 均非法）
fn split_path(path: &str) -> Result<Vec<String>, String> {
    let p = path.trim();
    if p.is_empty() || p.starts_with('/') || p.ends_with('/') {
        return Err(format!("非法路径: {path}"));
    }
    let mut segs = Vec::new();
    for seg in p.split('/') {
        segs.push(categories::validate_name(seg)?);
    }
    Ok(segs)
}

/// upsert 单个分类（含缺失祖先补建）；返回 id
fn upsert_category(
    conn: &Connection,
    path: &str,
    sort_order: i64,
    rep: &mut ImportReport,
) -> Result<i64, String> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM categories WHERE path = ?1",
            params![path],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| format!("查询分类失败: {e}"))?
    {
        // 已存在：不改名不改序（doc 内条目由 import_doc 主循环按差异更新）
        return Ok(id);
    }
    let segs = split_path(path)?;
    let name = segs.last().cloned().unwrap_or_default();
    let parent_id = if segs.len() > 1 {
        let parent_path = segs[..segs.len() - 1].join("/");
        Some(upsert_category(conn, &parent_path, 0, rep)?)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO categories (parent_id, name, path, sort_order) VALUES (?1, ?2, ?3, ?4)",
        params![parent_id, name, path, sort_order],
    )
    .map_err(|e| format!("创建分类失败: {e}"))?;
    rep.categories_created += 1;
    Ok(conn.last_insert_rowid())
}

pub fn import_doc(conn: &mut Connection, doc: &PortDoc) -> Result<ImportReport, String> {
    let mut rep = ImportReport::default();
    let tx = conn
        .transaction()
        .map_err(|e| format!("开启事务失败: {e}"))?;
    let inner = (|| -> Result<ImportReport, String> {
        let mut cats = doc.categories.clone();
        cats.sort_by_key(|c| split_path(&c.path).map(|s| s.len()).unwrap_or(usize::MAX));
        for c in &cats {
            let path = c.path.trim().to_string();
            if let Err(e) = split_path(&path) {
                rep.skipped.push(format!("分类 {path}: {e}"));
                continue;
            }
            let segs = split_path(&path).expect("已校验");
            let name = segs.last().cloned().unwrap_or_default();
            let existing: Option<(String, i64)> = tx
                .query_row(
                    "SELECT name, sort_order FROM categories WHERE path = ?1",
                    params![path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("查询分类失败: {e}"))?;
            match existing {
                Some((ename, esort)) => {
                    if ename != name || esort != c.sort_order {
                        tx.execute(
                            "UPDATE categories SET name = ?1, sort_order = ?2 WHERE path = ?3",
                            params![name, c.sort_order, path],
                        )
                        .map_err(|e| format!("更新分类失败: {e}"))?;
                        rep.categories_updated += 1;
                    }
                }
                None => {
                    upsert_category(&tx, &path, c.sort_order, &mut rep)?;
                }
            }
        }

        for t in &doc.tags {
            let name = match tags::validate_tag_name(&t.name) {
                Ok(n) => n,
                Err(e) => {
                    rep.skipped.push(format!("标签 {}: {e}", t.name));
                    continue;
                }
            };
            let tag_id = match tx
                .query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()
                .map_err(|e| format!("查询标签失败: {e}"))?
            {
                Some(id) => id,
                None => {
                    tx.execute("INSERT INTO tags (name) VALUES (?1)", params![name])
                        .map_err(|e| format!("创建标签失败: {e}"))?;
                    rep.tags_created += 1;
                    tx.last_insert_rowid()
                }
            };
            for a in &t.aliases {
                let alias = match tags::validate_tag_name(a) {
                    Ok(x) => x,
                    Err(e) => {
                        rep.skipped.push(format!("别名 {a}: {e}"));
                        continue;
                    }
                };
                if alias == name {
                    continue;
                }
                let owner: Option<i64> = tx
                    .query_row(
                        "SELECT tag_id FROM tag_aliases WHERE alias = ?1",
                        params![alias],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| format!("查询别名失败: {e}"))?;
                match owner {
                    Some(oid) if oid == tag_id => {}
                    Some(_) => {
                        rep.skipped.push(format!("别名 {alias} 已被其他标签占用"));
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO tag_aliases (alias, tag_id) VALUES (?1, ?2)",
                            params![alias, tag_id],
                        )
                        .map_err(|e| format!("创建别名失败: {e}"))?;
                        rep.aliases_added += 1;
                    }
                }
            }
        }
        Ok(rep)
    })();
    match inner {
        Ok(rep) => {
            tx.commit().map_err(|e| format!("提交失败: {e}"))?;
            Ok(rep)
        }
        Err(e) => Err(e),
    }
}

pub fn import_json(conn: &mut Connection, json: &str) -> Result<ImportReport, String> {
    let doc: PortDoc = serde_json::from_str(json).map_err(|e| format!("JSON 解析失败: {e}"))?;
    if doc.version > PORT_VERSION {
        return Err(format!(
            "导出文件版本 {} 高于当前支持的 {PORT_VERSION}",
            doc.version
        ));
    }
    import_doc(conn, &doc)
}
