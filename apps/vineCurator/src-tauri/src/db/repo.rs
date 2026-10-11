use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingEntry {
    pub key: String,
    pub value: String,
}

pub fn get_settings(conn: &Connection) -> Result<Vec<SettingEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT \"key\", \"value\" FROM settings ORDER BY \"key\"")?;
    let rows = stmt.query_map([], |r| {
        Ok(SettingEntry {
            key: r.get(0)?,
            value: r.get(1)?,
        })
    })?;
    rows.collect()
}

pub fn save_settings(
    conn: &mut Connection,
    entries: &[SettingEntry],
) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;
    for e in entries {
        tx.execute(
            "INSERT INTO settings (\"key\", \"value\") VALUES (?1, ?2)
             ON CONFLICT(\"key\") DO UPDATE SET \"value\" = excluded.\"value\"",
            params![e.key, e.value],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------- roots

#[derive(Debug, Clone, Serialize)]
pub struct RootEntry {
    pub id: i64,
    pub path: String,
    pub label: String,
    pub item_count: i64,
    pub created_at: String,
}

/// 统一路径形态用于比较：去首尾空白、/ → \、去重复尾反斜杠（保留盘根 "d:\"）、Windows 大小写不敏感
pub fn normalize_root_path(path: &str) -> String {
    let mut p = path.trim().replace('/', "\\");
    while p.len() > 3 && p.ends_with('\\') {
        p.pop();
    }
    p.to_lowercase()
}

/// 两个根目录路径是否冲突（相同或互为祖先）
pub fn paths_conflict(a: &str, b: &str) -> bool {
    let a = normalize_root_path(a);
    let b = normalize_root_path(b);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.starts_with(&format!("{b}\\")) || b.starts_with(&format!("{a}\\"))
}

fn ensure_dir(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("根目录路径不能为空".into());
    }
    let meta = std::fs::metadata(path).map_err(|_| format!("目录不存在或不可访问: {path}"))?;
    if !meta.is_dir() {
        return Err(format!("不是目录: {path}"));
    }
    Ok(())
}

fn root_paths(conn: &Connection) -> Result<Vec<(i64, String)>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT id, path FROM roots")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect()
}

fn get_root(conn: &Connection, id: i64) -> Result<RootEntry, String> {
    conn.query_row(
        "SELECT r.id, r.path, r.label, r.created_at,
                (SELECT COUNT(*) FROM items i WHERE i.root_id = r.id)
         FROM roots r WHERE r.id = ?1",
        [id],
        |r| {
            Ok(RootEntry {
                id: r.get(0)?,
                path: r.get(1)?,
                label: r.get(2)?,
                created_at: r.get(3)?,
                item_count: r.get(4)?,
            })
        },
    )
    .map_err(|_| "根目录不存在".to_string())
}

pub fn list_roots(conn: &Connection) -> Result<Vec<RootEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.path, r.label, r.created_at,
                (SELECT COUNT(*) FROM items i WHERE i.root_id = r.id)
         FROM roots r ORDER BY r.path",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RootEntry {
            id: r.get(0)?,
            path: r.get(1)?,
            label: r.get(2)?,
            created_at: r.get(3)?,
            item_count: r.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn add_root(conn: &mut Connection, path: &str, label: &str) -> Result<RootEntry, String> {
    let p = path.trim();
    ensure_dir(p)?;
    for (_, existing) in root_paths(conn).map_err(|e| format!("读取根目录失败: {e}"))? {
        if paths_conflict(p, &existing) {
            return Err(format!("与已有根目录重叠: {existing}"));
        }
    }
    conn.execute(
        "INSERT INTO roots (path, label) VALUES (?1, ?2)",
        params![p, label.trim()],
    )
    .map_err(|e| format!("添加根目录失败: {e}"))?;
    get_root(conn, conn.last_insert_rowid())
}

pub fn remove_root(conn: &mut Connection, id: i64) -> Result<(), String> {
    let n = conn
        .execute("DELETE FROM roots WHERE id = ?1", [id])
        .map_err(|e| format!("删除根目录失败: {e}"))?;
    if n == 0 {
        return Err("根目录不存在".into());
    }
    Ok(())
}

/// 读取单个设置值（不存在返回空串）
pub fn get_setting_value(conn: &Connection, key: &str) -> Result<String, rusqlite::Error> {
    conn.query_row(
        "SELECT \"value\" FROM settings WHERE \"key\" = ?1",
        [key],
        |r| r.get(0),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(String::new()),
        other => Err(other),
    })
}

/// 读取根目录磁盘路径（扫描/哈希 worker 用）
pub fn get_root_path(conn: &Connection, id: i64) -> Result<String, String> {
    conn.query_row("SELECT path FROM roots WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|_| format!("根目录不存在: {id}"))
}

pub fn redirect_root(conn: &mut Connection, id: i64, new_path: &str) -> Result<RootEntry, String> {
    let p = new_path.trim();
    ensure_dir(p)?;
    for (other_id, existing) in root_paths(conn).map_err(|e| format!("读取根目录失败: {e}"))?
    {
        if other_id != id && paths_conflict(p, &existing) {
            return Err(format!("与已有根目录重叠: {existing}"));
        }
    }
    let n = conn
        .execute("UPDATE roots SET path = ?1 WHERE id = ?2", params![p, id])
        .map_err(|e| format!("重定向失败: {e}"))?;
    if n == 0 {
        return Err("根目录不存在".into());
    }
    get_root(conn, id)
}
