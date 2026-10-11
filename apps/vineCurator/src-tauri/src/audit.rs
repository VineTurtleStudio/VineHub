use rusqlite::{params, Connection};
use serde::Serialize;

/// 审计动作白名单（凭据类操作不记 detail 内容）
pub const ACTIONS: [&str; 9] = [
    "create", "update", "delete", "run", "cancel", "login", "logout", "setup", "password",
];

#[derive(Debug, Clone, Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub action: String,
    pub entity: String,
    pub entity_id: Option<String>,
    pub detail: String,
    pub created_at: String,
}

/// 写入一条审计（所有写操作的统一出口；detail 必须是 JSON 字符串且不得含密码）
pub fn record(
    conn: &Connection,
    user_id: Option<i64>,
    action: &str,
    entity: &str,
    entity_id: Option<&str>,
    detail: &str,
) -> Result<(), String> {
    if !ACTIONS.contains(&action) {
        return Err(format!("非法审计动作: {action}"));
    }
    if entity.trim().is_empty() {
        return Err("审计实体不能为空".into());
    }
    conn.execute(
        "INSERT INTO audit_log (user_id, action, entity, entity_id, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, action, entity, entity_id, detail],
    )
    .map_err(|e| format!("写入审计失败: {e}"))?;
    Ok(())
}

/// 最近审计（倒序），可按实体过滤；limit 钳制 1..=500
pub fn recent(
    conn: &Connection,
    limit: i64,
    entity: Option<&str>,
) -> Result<Vec<AuditRow>, String> {
    let limit = limit.clamp(1, 500);
    let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<AuditRow> {
        Ok(AuditRow {
            id: r.get(0)?,
            user_id: r.get(1)?,
            username: r.get(2)?,
            action: r.get(3)?,
            entity: r.get(4)?,
            entity_id: r.get(5)?,
            detail: r.get(6)?,
            created_at: r.get(7)?,
        })
    };
    let sql_base = "SELECT a.id, a.user_id, u.username, a.action, a.entity, a.entity_id, a.detail, a.created_at
         FROM audit_log a LEFT JOIN users u ON u.id = a.user_id";
    let mut out = Vec::new();
    match entity {
        Some(e) if !e.trim().is_empty() => {
            let sql = format!("{sql_base} WHERE a.entity = ?1 ORDER BY a.id DESC LIMIT ?2");
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| format!("查询审计失败: {e}"))?;
            let rows = stmt
                .query_map(params![e, limit], map_row)
                .map_err(|e| format!("查询审计失败: {e}"))?;
            for row in rows {
                out.push(row.map_err(|e| format!("读取审计失败: {e}"))?);
            }
        }
        _ => {
            let sql = format!("{sql_base} ORDER BY a.id DESC LIMIT ?1");
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| format!("查询审计失败: {e}"))?;
            let rows = stmt
                .query_map(params![limit], map_row)
                .map_err(|e| format!("查询审计失败: {e}"))?;
            for row in rows {
                out.push(row.map_err(|e| format!("读取审计失败: {e}"))?);
            }
        }
    }
    Ok(out)
}
