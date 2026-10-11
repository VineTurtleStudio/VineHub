use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::sync::{Mutex, MutexGuard};

/// 角色白名单（阶段四/远端接口的执行边界；本地本期不做命令门禁）
pub const ROLES: [&str; 3] = ["admin", "editor", "viewer"];

#[derive(Debug, Clone, Serialize)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub created_at: String,
}

fn validate_username(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("用户名不能为空".into());
    }
    if n.chars().count() > 64 {
        return Err("用户名过长（≤64 字符）".into());
    }
    if n.chars().any(|c| c.is_whitespace()) {
        return Err("用户名不能含空白字符".into());
    }
    Ok(n.to_string())
}

fn validate_password(pw: &str) -> Result<(), String> {
    if pw.chars().count() < 6 {
        return Err("密码至少 6 位".into());
    }
    Ok(())
}

fn validate_role(role: &str) -> Result<(), String> {
    if !ROLES.contains(&role) {
        return Err(format!("非法角色: {role}"));
    }
    Ok(())
}

/// Argon2id 哈希（PHC 字符串格式，含随机盐）
pub fn hash_password(plain: &str) -> Result<String, String> {
    validate_password(plain)?;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("密码哈希失败: {e}"))
}

pub fn verify_password(plain: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(plain.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub fn count_users(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .map_err(|e| format!("统计用户失败: {e}"))
}

fn count_admins(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin'", [], |r| {
        r.get(0)
    })
    .map_err(|e| format!("统计管理员失败: {e}"))
}

fn user_role(conn: &Connection, id: i64) -> Result<Option<String>, String> {
    conn.query_row("SELECT role FROM users WHERE id = ?1", [id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("查询用户失败: {e}"))
}

/// 首次启动创建初始管理员（仅 users 为空时允许）
pub fn setup_admin(conn: &Connection, username: &str, password: &str) -> Result<i64, String> {
    if count_users(conn)? > 0 {
        return Err("已存在用户，请用「创建用户」".into());
    }
    create_user(conn, username, password, "admin")
}

pub fn create_user(
    conn: &Connection,
    username: &str,
    password: &str,
    role: &str,
) -> Result<i64, String> {
    let name = validate_username(username)?;
    validate_password(password)?;
    validate_role(role)?;
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM users WHERE username = ?1", [&name], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| format!("查询用户失败: {e}"))?;
    if exists.is_some() {
        return Err(format!("用户名已存在: {name}"));
    }
    let hash = hash_password(password)?;
    conn.execute(
        "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
        params![name, hash, role],
    )
    .map_err(|e| format!("创建用户失败: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// 登录校验：成功返回用户，失败 None（不区分用户不存在/密码错误）
pub fn authenticate(
    conn: &Connection,
    username: &str,
    password: &str,
) -> Result<Option<UserRow>, String> {
    let row: Option<(i64, String, String, String, String)> = conn
        .query_row(
            "SELECT id, username, password_hash, role, created_at FROM users WHERE username = ?1",
            [username.trim()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| format!("查询用户失败: {e}"))?;
    match row {
        None => Ok(None),
        Some((id, username, hash, role, created_at)) => {
            if verify_password(password, &hash) {
                Ok(Some(UserRow {
                    id,
                    username,
                    role,
                    created_at,
                }))
            } else {
                Ok(None)
            }
        }
    }
}

pub fn list_users(conn: &Connection) -> Result<Vec<UserRow>, String> {
    let mut stmt = conn
        .prepare("SELECT id, username, role, created_at FROM users ORDER BY id")
        .map_err(|e| format!("查询用户失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(UserRow {
                id: r.get(0)?,
                username: r.get(1)?,
                role: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map_err(|e| format!("查询用户失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取用户失败: {e}"))?);
    }
    Ok(out)
}

pub fn delete_user(conn: &Connection, id: i64) -> Result<(), String> {
    let role = user_role(conn, id)?.ok_or_else(|| format!("用户不存在: {id}"))?;
    if role == "admin" && count_admins(conn)? <= 1 {
        return Err("至少保留一个管理员".into());
    }
    conn.execute("DELETE FROM users WHERE id = ?1", [id])
        .map_err(|e| format!("删除用户失败: {e}"))?;
    Ok(())
}

pub fn set_role(conn: &Connection, id: i64, role: &str) -> Result<(), String> {
    validate_role(role)?;
    let old = user_role(conn, id)?.ok_or_else(|| format!("用户不存在: {id}"))?;
    if old == "admin" && role != "admin" && count_admins(conn)? <= 1 {
        return Err("至少保留一个管理员".into());
    }
    conn.execute(
        "UPDATE users SET role = ?1 WHERE id = ?2",
        params![role, id],
    )
    .map_err(|e| format!("修改角色失败: {e}"))?;
    Ok(())
}

pub fn change_password(conn: &Connection, id: i64, old: &str, new: &str) -> Result<(), String> {
    validate_password(new)?;
    let hash: String = conn
        .query_row("SELECT password_hash FROM users WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| format!("查询用户失败: {e}"))?
        .ok_or_else(|| format!("用户不存在: {id}"))?;
    if !verify_password(old, &hash) {
        return Err("旧密码不正确".into());
    }
    let new_hash = hash_password(new)?;
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![new_hash, id],
    )
    .map_err(|e| format!("修改密码失败: {e}"))?;
    Ok(())
}

/// 登录会话（进程内存，随应用退出失效）；poison 自行恢复
#[derive(Debug, Default)]
pub struct AuthState {
    session: Mutex<Option<UserRow>>,
}

impl AuthState {
    fn lock(&self) -> MutexGuard<'_, Option<UserRow>> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn login(&self, user: UserRow) {
        *self.lock() = Some(user);
    }

    pub fn logout(&self) {
        *self.lock() = None;
    }

    pub fn current(&self) -> Option<UserRow> {
        self.lock().clone()
    }

    pub fn user_id(&self) -> Option<i64> {
        self.lock().as_ref().map(|u| u.id)
    }
}
