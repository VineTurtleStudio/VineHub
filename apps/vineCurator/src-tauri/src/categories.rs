//! 分类树 CRUD（纯逻辑，零 Tauri）：物化路径 `path` 全局唯一，父先于子。
//! 约定：分隔符 `/`，名称不含 `/`；改名 / 移动在事务内改写自身 + 全部子孙路径。

use rusqlite::{params, Connection, OptionalExtension};

const MAX_NAME: usize = 100;

/// 名称归一化校验：trim 后非空、不含路径分隔符、长度受限
pub fn validate_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("名称不能为空".into());
    }
    if n.contains('/') {
        return Err("名称不能包含 /".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("名称过长（最多 {MAX_NAME} 字符）"));
    }
    Ok(n.to_string())
}

struct NodePath {
    path: String,
}

fn get_node(conn: &Connection, id: i64) -> Result<NodePath, String> {
    conn.query_row("SELECT path FROM categories WHERE id = ?1", [id], |r| {
        Ok(NodePath { path: r.get(0)? })
    })
    .optional()
    .map_err(|e| format!("读取分类失败: {e}"))
    .and_then(|o| o.ok_or_else(|| format!("分类不存在: {id}")))
}

fn parent_path(conn: &Connection, parent_id: Option<i64>) -> Result<Option<String>, String> {
    match parent_id {
        None => Ok(None),
        Some(pid) => Ok(Some(get_node(conn, pid)?.path)),
    }
}

/// 拼接新路径：父路径存在则 `父/名`，否则 `名`
fn join_path(parent: &Option<String>, name: &str) -> String {
    match parent {
        Some(p) => format!("{p}/{name}"),
        None => name.to_string(),
    }
}

/// 物化路径校验：逐段 `validate_name`（空段忽略），返回归一化 path。
/// 供 `ensure_path` / 规则 `set_category` / 自动分类在写入前校验。
pub fn validate_path_str(path: &str) -> Result<String, String> {
    let parts: Vec<&str> = path
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("路径不能为空".into());
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        out.push(validate_name(p)?);
    }
    Ok(out.join("/"))
}

/// 确保物化路径存在（缺哪级建哪级，复用已存在节点），返回叶节点 id。
/// 全程 path 唯一约束兜底，事务内可安全重复调用。
pub fn ensure_path(conn: &Connection, path: &str) -> Result<i64, String> {
    let norm = validate_path_str(path)?;
    let mut parent_id: Option<i64> = None;
    let mut acc = String::new();
    let mut leaf = 0i64;
    for seg in norm.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM categories WHERE path = ?1", [&acc], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| format!("查询分类失败: {e}"))?;
        leaf = match existing {
            Some(id) => id,
            None => {
                conn.execute(
                    "INSERT INTO categories (parent_id, name, path) VALUES (?1, ?2, ?3)
                     ON CONFLICT(path) DO NOTHING",
                    rusqlite::params![parent_id, seg, acc],
                )
                .map_err(|e| format!("创建分类失败: {e}"))?;
                conn.query_row("SELECT id FROM categories WHERE path = ?1", [&acc], |r| {
                    r.get(0)
                })
                .map_err(|e| format!("读取分类失败: {e}"))?
            }
        };
        parent_id = Some(leaf);
    }
    Ok(leaf)
}

/// 同级重名检查（parent_id 用 `IS` 兼容 NULL）
fn sibling_exists(
    conn: &Connection,
    parent_id: Option<i64>,
    name: &str,
    exclude_id: Option<i64>,
) -> Result<bool, String> {
    let sql = if exclude_id.is_some() {
        "SELECT 1 FROM categories WHERE parent_id IS ?1 AND name = ?2 AND id != ?3"
    } else {
        "SELECT 1 FROM categories WHERE parent_id IS ?1 AND name = ?2"
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("检查同级重名失败: {e}"))?;
    let found = if let Some(ex) = exclude_id {
        stmt.exists(params![parent_id, name, ex])
    } else {
        stmt.exists(params![parent_id, name])
    }
    .map_err(|e| format!("检查同级重名失败: {e}"))?;
    Ok(found)
}

/// 创建分类，返回新 id
pub fn create_category(
    conn: &mut Connection,
    parent_id: Option<i64>,
    name: &str,
) -> Result<i64, String> {
    let name = validate_name(name)?;
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<i64, String> {
        let parent = parent_path(&tx, parent_id)?;
        if sibling_exists(&tx, parent_id, &name, None)? {
            return Err("同级已存在同名分类".into());
        }
        let path = join_path(&parent, &name);
        tx.execute(
            "INSERT INTO categories (parent_id, name, path) VALUES (?1, ?2, ?3)",
            params![parent_id, name, path],
        )
        .map_err(|e| format!("创建分类失败: {e}"))?;
        Ok(tx.last_insert_rowid())
    })();
    match inner {
        Ok(id) => {
            tx.commit().map_err(|e| format!("提交失败: {e}"))?;
            Ok(id)
        }
        Err(e) => Err(e),
    }
}

/// 改名：校验同级重名，事务内改写自身 + 全部子孙路径
pub fn rename_category(conn: &mut Connection, id: i64, name: &str) -> Result<(), String> {
    let name = validate_name(name)?;
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<(), String> {
        let node = get_node(&tx, id)?;
        let parent_id: Option<i64> = tx
            .query_row(
                "SELECT parent_id FROM categories WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| format!("读取父分类失败: {e}"))?;
        if sibling_exists(&tx, parent_id, &name, Some(id))? {
            return Err("同级已存在同名分类".into());
        }
        let parent = parent_path(&tx, parent_id)?;
        let new_path = join_path(&parent, &name);
        rewrite_subtree(&tx, &node.path, &new_path)?;
        tx.execute(
            "UPDATE categories SET name = ?1 WHERE id = ?2",
            params![name, id],
        )
        .map_err(|e| format!("改名失败: {e}"))?;
        Ok(())
    })();
    match inner {
        Ok(()) => tx.commit().map_err(|e| format!("提交失败: {e}")),
        Err(e) => Err(e),
    }
}

/// 移动到新父级（None = 顶层）。拒绝：移到自身 / 自己的子孙下 / 目标同级重名。
pub fn move_category(
    conn: &mut Connection,
    id: i64,
    new_parent_id: Option<i64>,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<(), String> {
        let node = get_node(&tx, id)?;
        if new_parent_id == Some(id) {
            return Err("不能移动到自身下".into());
        }
        let new_parent = parent_path(&tx, new_parent_id)?;
        // 防环：目标父路径必须不位于当前子树内
        if let Some(pp) = &new_parent {
            if *pp == node.path || pp.starts_with(&format!("{}/", node.path)) {
                return Err("不能移动到自己的子孙下".into());
            }
        }
        if sibling_exists(&tx, new_parent_id, &node_path_name(&tx, id)?, Some(id))? {
            return Err("目标同级已存在同名分类".into());
        }
        let name = node_path_name(&tx, id)?;
        let new_path = join_path(&new_parent, &name);
        if new_path == node.path {
            return Ok(());
        }
        // 全量冲突预检：子孙在改写前仍持有旧路径，且新旧前缀不可能交错
        // （新父级已在上面排除位于本子树内），故直接查全局唯一性即可
        let descendants = descendant_paths(&tx, &node.path)?;
        for old in std::iter::once(node.path.clone()).chain(descendants.clone()) {
            let np = format!("{new_path}{}", &old[node.path.len()..]);
            let clash = tx
                .query_row("SELECT 1 FROM categories WHERE path = ?1", [&np], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()
                .map_err(|e| format!("冲突检查失败: {e}"))?;
            if clash.is_some() {
                return Err(format!("目标路径已存在: {np}"));
            }
        }
        rewrite_subtree(&tx, &node.path, &new_path)?;
        // parent_id 同步
        tx.execute(
            "UPDATE categories SET parent_id = ?1 WHERE id = ?2",
            params![new_parent_id, id],
        )
        .map_err(|e| format!("更新父级失败: {e}"))?;
        Ok(())
    })();
    match inner {
        Ok(()) => tx.commit().map_err(|e| format!("提交失败: {e}")),
        Err(e) => Err(e),
    }
}

/// 删除分类：级联子孙（FK）与条目关联；返回被删数量
pub fn delete_category(conn: &Connection, id: i64) -> Result<usize, String> {
    get_node(conn, id)?;
    conn.execute("DELETE FROM categories WHERE id = ?1", [id])
        .map_err(|e| format!("删除分类失败: {e}"))
}

// ---------------------------------------------------------------- 内部工具

fn node_path_name(conn: &Connection, id: i64) -> Result<String, String> {
    conn.query_row("SELECT name FROM categories WHERE id = ?1", [id], |r| {
        r.get(0)
    })
    .map_err(|e| format!("读取分类名失败: {e}"))
}

/// 直接子孙路径（不含自身，按 path 字典序）
fn descendant_paths(conn: &Connection, path: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT path FROM categories WHERE path LIKE ?1 ESCAPE '\\' ORDER BY path")
        .map_err(|e| format!("读取子孙失败: {e}"))?;
    let prefix = like_prefix(&format!("{path}/"));
    let rows = stmt
        .query_map([&prefix], |r| r.get::<_, String>(0))
        .map_err(|e| format!("读取子孙失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取子孙失败: {e}"))?);
    }
    Ok(out)
}

/// 改写前缀：所有 `旧前缀` 开头的 path 换成 `新前缀`（逐行，避免 LIKE 语义问题）
fn rewrite_subtree(conn: &Connection, old_prefix: &str, new_prefix: &str) -> Result<(), String> {
    let paths = descendant_paths(conn, old_prefix)?;
    conn.execute(
        "UPDATE categories SET path = ?1 WHERE path = ?2",
        params![new_prefix, old_prefix],
    )
    .map_err(|e| format!("改写路径失败: {e}"))?;
    for p in paths {
        let suffix = &p[old_prefix.len()..];
        conn.execute(
            "UPDATE categories SET path = ?1 WHERE path = ?2",
            params![format!("{new_prefix}{suffix}"), p],
        )
        .map_err(|e| format!("改写子孙路径失败: {e}"))?;
    }
    Ok(())
}

/// `LIKE` 前缀字面量化（转义 `\ % _`）
pub fn like_prefix(s: &str) -> String {
    format!(
        "{}%",
        s.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_bad_names() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a/b").is_err());
        assert_eq!(validate_name("  爵士  ").unwrap(), "爵士");
        assert!(validate_name(&"长".repeat(101)).is_err());
    }

    #[test]
    fn like_prefix_escapes_wildcards() {
        assert_eq!(like_prefix("50%_off"), "50\\%\\_off%");
    }

    #[test]
    fn validate_path_str_normalizes_segments() {
        assert_eq!(validate_path_str("  音乐 / 摇滚 ").unwrap(), "音乐/摇滚");
        assert!(validate_path_str("").is_err());
        assert!(validate_path_str("///").is_err());
        assert!(validate_path_str(&format!("a/{}", "长".repeat(101))).is_err());
        assert!(validate_path_str("a//b").unwrap() == "a/b");
    }

    #[test]
    fn ensure_path_creates_ancestors_and_reuses() {
        let mut conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrate::migrate(&mut conn).expect("迁移");
        let leaf = ensure_path(&conn, "音乐/摇滚/英伦").expect("建链");
        let (path, parent): (String, Option<i64>) = conn
            .query_row(
                "SELECT path, parent_id FROM categories WHERE id = ?1",
                [leaf],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("读叶");
        assert_eq!(path, "音乐/摇滚/英伦");
        let parent_path: String = conn
            .query_row(
                "SELECT path FROM categories WHERE id = ?1",
                [parent.unwrap()],
                |r| r.get(0),
            )
            .expect("读父");
        assert_eq!(parent_path, "音乐/摇滚");
        // 复用：同路径再次 ensure → 同一 id，零新增
        let again = ensure_path(&conn, "音乐/摇滚/英伦").expect("复用");
        assert_eq!(again, leaf);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }
}
