use crate::audit;
use crate::auth::{self, AuthState, UserRow};
use crate::browse;
use crate::categories;
use crate::db::{repo, DbState};
use crate::env::{self, ToolsReport};
use crate::scan::{self, ScanProgress};
use crate::tags;
use crate::tasks::{self, HashProgress};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};

#[derive(Debug, Default)]
pub struct JobState {
    flag: Mutex<Option<Arc<AtomicBool>>>,
}

impl JobState {
    /// 获取当前互斥锁（poison 自行恢复）
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<AtomicBool>>> {
        self.flag.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 开新任务：已有任务运行中返回 Err，否则登记取消标志
    fn begin_job(&self) -> Result<Arc<AtomicBool>, String> {
        let mut slot = self.lock();
        if let Some(existing) = slot.as_ref() {
            if !existing.load(Ordering::Relaxed) {
                return Err("已有任务在运行".into());
            }
        }
        let flag = Arc::new(AtomicBool::new(false));
        *slot = Some(flag.clone());
        Ok(flag)
    }

    fn end_job(&self, flag: &AtomicBool) {
        let mut slot = self.lock();
        if let Some(cur) = slot.as_ref() {
            if std::ptr::eq(cur.as_ref(), flag) {
                *slot = None;
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DbInfo {
    pub path: String,
}

/// 写操作统一审计入口（覆盖检查见 tests/audit_test.rs）：
/// action 走 audit::ACTIONS 白名单，entity_id 序列化为 TEXT，detail 为 JSON 且不得含密码
fn audit_now(
    conn: &rusqlite::Connection,
    auth: &State<'_, AuthState>,
    action: &str,
    entity: &str,
    entity_id: Option<i64>,
    detail: serde_json::Value,
) -> Result<(), String> {
    audit::record(
        conn,
        auth.user_id(),
        action,
        entity,
        entity_id.map(|v| v.to_string()).as_deref(),
        &detail.to_string(),
    )
}

/// 后台任务完成审计（线程内无 State，user 在启动时捕获）
fn audit_job(
    conn: &rusqlite::Connection,
    user: Option<i64>,
    kind: &str,
    detail: serde_json::Value,
) {
    if let Err(e) = audit::record(conn, user, "run", "job", Some(kind), &detail.to_string()) {
        eprintln!("[vineCurator] 任务审计写入失败: {e}");
    }
}

#[tauri::command(async)]
pub fn probe_tools(state: State<'_, DbState>) -> Result<ToolsReport, String> {
    let overrides = tool_overrides(&state);
    Ok(env::probe_tools(&overrides))
}

#[tauri::command]
pub fn get_db_info(state: State<'_, DbState>) -> DbInfo {
    DbInfo {
        path: state.db_path.to_string_lossy().into_owned(),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, DbState>) -> Result<Vec<repo::SettingEntry>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    repo::get_settings(&conn).map_err(|e| format!("读取设置失败: {e}"))
}

#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    entries: Vec<repo::SettingEntry>,
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    repo::save_settings(&mut conn, &entries).map_err(|e| format!("保存设置失败: {e}"))?;
    // 派生目录变更 → 重新加入 asset 协议白名单（缩略图 <img> 需要）
    for e in &entries {
        if e.key == "derived_dir" && !e.value.trim().is_empty() {
            let thumbs_dir = std::path::Path::new(e.value.trim()).join("thumbs");
            if let Err(err) = app
                .asset_protocol_scope()
                .allow_directory(&thumbs_dir, true)
            {
                eprintln!("[vineCurator] 缩略图目录 asset 白名单失败: {err}");
            }
        }
    }
    let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    audit_now(
        &conn,
        &auth,
        "update",
        "settings",
        None,
        serde_json::json!({ "keys": keys }),
    )?;
    Ok(())
}

#[tauri::command]
pub fn list_roots(state: State<'_, DbState>) -> Result<Vec<repo::RootEntry>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    repo::list_roots(&conn).map_err(|e| format!("读取根目录失败: {e}"))
}

#[tauri::command]
pub fn add_root(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    path: String,
    label: Option<String>,
) -> Result<repo::RootEntry, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let root = repo::add_root(&mut conn, &path, label.as_deref().unwrap_or(""))?;
    if let Err(e) = app.asset_protocol_scope().allow_directory(&path, true) {
        eprintln!("[vineCurator] 根目录 asset 白名单失败: {e}");
    }
    audit_now(
        &conn,
        &auth,
        "create",
        "root",
        Some(root.id),
        serde_json::json!({ "path": path }),
    )?;
    Ok(root)
}

#[tauri::command]
pub fn remove_root(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    root_id: i64,
) -> Result<(), String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    repo::remove_root(&mut conn, root_id)?;
    crate::fts::prune_orphans(&conn)?;
    audit_now(
        &conn,
        &auth,
        "delete",
        "root",
        Some(root_id),
        serde_json::json!({}),
    )?;
    Ok(())
}

#[tauri::command]
pub fn redirect_root(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    root_id: i64,
    new_path: String,
) -> Result<repo::RootEntry, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let root = repo::redirect_root(&mut conn, root_id, &new_path)?;
    if let Err(e) = app.asset_protocol_scope().allow_directory(&new_path, true) {
        eprintln!("[vineCurator] 根目录 asset 白名单失败: {e}");
    }
    audit_now(
        &conn,
        &auth,
        "update",
        "root",
        Some(root_id),
        serde_json::json!({ "new_path": new_path }),
    )?;
    Ok(root)
}

#[tauri::command(async)]
pub fn start_scan(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    root_id: i64,
) -> Result<(), String> {
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut conn = match pool.get() {
                Ok(c) => c,
                Err(e) => {
                    let _ = app.emit(
                        "scan-progress",
                        ScanProgress::failure(format!("数据库连接失败: {e}")),
                    );
                    return;
                }
            };
            let root_path: PathBuf = match repo::get_root_path(&conn, root_id) {
                Ok(p) => PathBuf::from(p),
                Err(e) => {
                    let _ = app.emit("scan-progress", ScanProgress::failure(e));
                    return;
                }
            };
            let mut emit = |p: &ScanProgress| {
                let _ = app.emit("scan-progress", p);
            };
            match scan::scan_root(&mut conn, root_id, &root_path, &cancel, &mut emit) {
                Ok(_) => {}
                Err(e) => {
                    let _ = app.emit("scan-progress", ScanProgress::failure(e));
                }
            }
        }));
        if result.is_err() {
            let _ = app2.emit("scan-progress", ScanProgress::failure("扫描线程崩溃"));
        }
        // 完成审计（扫描本身不逐文件记账，按任务汇总）
        if let Ok(conn) = pool.get() {
            let status = if cancel.load(Ordering::Relaxed) {
                "cancelled"
            } else if result.is_err() {
                "failed"
            } else {
                "done"
            };
            audit_job(
                &conn,
                audit_user,
                "scan",
                serde_json::json!({ "root_id": root_id, "status": status }),
            );
        }
        // end_job 需要通过 app 重新获取 managed state（State 引用不能跨线程）
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

#[tauri::command(async)]
pub fn start_hash_job(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    root_id: Option<i64>,
) -> Result<(), String> {
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let conn = match pool.get() {
                Ok(c) => c,
                Err(e) => {
                    let _ = app.emit(
                        "hash-progress",
                        HashProgress::failure(format!("数据库连接失败: {e}")),
                    );
                    return;
                }
            };
            // 收集本次要处理的根目录（root_id 指定单个，否则全部）
            let roots: Vec<(i64, PathBuf)> = match root_id {
                Some(rid) => match repo::get_root_path(&conn, rid) {
                    Ok(p) => vec![(rid, PathBuf::from(p))],
                    Err(e) => {
                        let _ = app.emit("hash-progress", HashProgress::failure(e));
                        return;
                    }
                },
                None => match repo::list_roots(&conn) {
                    Ok(list) => list
                        .into_iter()
                        .map(|r| (r.id, PathBuf::from(r.path)))
                        .collect(),
                    Err(e) => {
                        let _ = app.emit(
                            "hash-progress",
                            HashProgress::failure(format!("读取根目录失败: {e}")),
                        );
                        return;
                    }
                },
            };
            // 先统一标 pending，再逐根哈希
            for (rid, _) in &roots {
                if let Err(e) = tasks::mark_pending(&conn, Some(*rid), &[]) {
                    let _ = app.emit("hash-progress", HashProgress::failure(e));
                    return;
                }
            }
            let mut emit = |p: &HashProgress| {
                let _ = app.emit("hash-progress", p);
            };
            for (_, root_path) in &roots {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = tasks::run_pending_hashes(&conn, root_path, &cancel, &mut emit) {
                    let _ = app.emit("hash-progress", HashProgress::failure(e));
                    return;
                }
            }
        }));
        if result.is_err() {
            let _ = app2.emit("hash-progress", HashProgress::failure("哈希线程崩溃"));
        }
        if let Ok(conn) = pool.get() {
            let status = if cancel.load(Ordering::Relaxed) {
                "cancelled"
            } else if result.is_err() {
                "failed"
            } else {
                "done"
            };
            audit_job(
                &conn,
                audit_user,
                "hash",
                serde_json::json!({ "root_id": root_id, "status": status }),
            );
        }
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_job(
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    state: State<'_, DbState>,
) -> Result<(), String> {
    let slot = jobs.lock();
    if let Some(flag) = slot.as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    drop(slot);
    if let Ok(conn) = state.pool.get() {
        audit_now(&conn, &auth, "cancel", "job", None, serde_json::json!({}))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 浏览查询

#[tauri::command]
pub fn list_items(
    state: State<'_, DbState>,
    filter: browse::ItemFilter,
) -> Result<browse::ItemPage, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    browse::list_items(&conn, &filter)
}

#[tauri::command]
pub fn list_categories(state: State<'_, DbState>) -> Result<Vec<browse::CategoryNode>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    browse::list_categories(&conn)
}

#[tauri::command]
pub fn get_overview(
    state: State<'_, DbState>,
    root_id: Option<i64>,
) -> Result<browse::Overview, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    browse::overview(&conn, root_id)
}

#[tauri::command]
pub fn get_item(state: State<'_, DbState>, item_id: i64) -> Result<browse::ItemDetail, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    browse::get_item(&conn, item_id)
}

#[tauri::command]
pub fn search_items(
    state: State<'_, DbState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<browse::ItemRow>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    browse::search_items(&conn, &query, limit)
}

/// 批量改字段：只接受白名单字段（status/rating/year），值走占位符；
/// rating 受 0..=5 CHECK 约束，year 可清空(null)。改动后同步 FTS（year 进 body）。
#[tauri::command]
pub fn set_item_fields(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    item_ids: Vec<i64>,
    status: Option<String>,
    rating: Option<i64>,
    year: Option<i64>,
) -> Result<usize, String> {
    if status.is_none() && rating.is_none() && year.is_none() {
        return Err("没有要修改的字段".into());
    }
    if let Some(s) = &status {
        const ALLOWED: [&str; 5] = ["inbox", "unwatched", "reading", "done", "dropped"];
        if !ALLOWED.contains(&s.as_str()) {
            return Err(format!("非法状态: {s}"));
        }
    }
    if let Some(r) = rating {
        if !(0..=5).contains(&r) {
            return Err(format!("评分须在 0..=5: {r}"));
        }
    }
    let mut sets: Vec<&str> = Vec::new();
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(s) = &status {
        sets.push("status = ?1");
        args.push(s.clone().into());
    }
    if let Some(r) = rating {
        sets.push(if args.is_empty() { "?1" } else { "?2" });
        args.push(r.into());
    }
    if let Some(y) = year {
        sets.push(if args.is_empty() {
            "?1"
        } else if args.len() == 1 {
            "?2"
        } else {
            "?3"
        });
        args.push(y.into());
    }
    let sql = format!(
        "UPDATE items SET {}, updated_at = datetime('now') WHERE id IN (SELECT value FROM json_each(?{}))",
        sets.join(", "),
        args.len() + 1
    );
    let ids_json = serde_json::to_string(&item_ids).map_err(|e| format!("序列化失败: {e}"))?;
    args.push(ids_json.into());
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let n = conn
        .execute(&sql, rusqlite::params_from_iter(args.iter()))
        .map_err(|e| format!("更新失败: {e}"))?;
    for id in &item_ids {
        let _ = crate::fts::sync_item(&conn, *id);
    }
    audit_now(
        &conn,
        &auth,
        "update",
        "item",
        None,
        serde_json::json!({ "count": n, "status": status, "rating": rating, "year": year }),
    )?;
    Ok(n)
}

/// 用系统默认应用打开（只读，不修改媒体文件）
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    crate::shell::open_path(&path)
}

/// 在资源管理器中定位
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    crate::shell::reveal_path(&path)
}

// ---------------------------------------------------------------- 分类树 CRUD

#[tauri::command]
pub fn create_category(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    parent_id: Option<i64>,
    name: String,
) -> Result<i64, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let id = categories::create_category(&mut conn, parent_id, &name)?;
    audit_now(
        &conn,
        &auth,
        "create",
        "category",
        Some(id),
        serde_json::json!({ "name": name }),
    )?;
    Ok(id)
}

#[tauri::command]
pub fn rename_category(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    category_id: i64,
    name: String,
) -> Result<(), String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    categories::rename_category(&mut conn, category_id, &name)?;
    audit_now(
        &conn,
        &auth,
        "update",
        "category",
        Some(category_id),
        serde_json::json!({ "name": name }),
    )?;
    Ok(())
}

#[tauri::command]
pub fn move_category(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    category_id: i64,
    parent_id: Option<i64>,
) -> Result<(), String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    categories::move_category(&mut conn, category_id, parent_id)?;
    audit_now(
        &conn,
        &auth,
        "update",
        "category",
        Some(category_id),
        serde_json::json!({ "parent_id": parent_id }),
    )?;
    Ok(())
}

#[tauri::command]
pub fn delete_category(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    category_id: i64,
) -> Result<(), String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    categories::delete_category(&conn, category_id)?;
    audit_now(
        &conn,
        &auth,
        "delete",
        "category",
        Some(category_id),
        serde_json::json!({}),
    )?;
    Ok(())
}

/// 条目 ↔ 分类：整体替换为给定集合（事务）
#[tauri::command]
pub fn set_item_categories(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    item_id: i64,
    category_ids: Vec<i64>,
) -> Result<(), String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let count = category_ids.len();
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<(), String> {
        let n: i64 = tx
            .query_row("SELECT 1 FROM items WHERE id = ?1", [item_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("查询条目失败: {e}"))?
            .unwrap_or(0);
        if n == 0 {
            return Err(format!("条目不存在: {item_id}"));
        }
        tx.execute("DELETE FROM item_categories WHERE item_id = ?1", [item_id])
            .map_err(|e| format!("清除分类关联失败: {e}"))?;
        for cid in category_ids {
            tx.execute(
                "INSERT INTO item_categories (item_id, category_id, source) VALUES (?1, ?2, 'manual')
                 ON CONFLICT DO NOTHING",
                rusqlite::params![item_id, cid],
            )
            .map_err(|e| format!("关联分类失败: {e}"))?;
        }
        Ok(())
    })();
    match inner {
        Ok(()) => {
            tx.commit().map_err(|e| format!("提交失败: {e}"))?;
            audit_now(
                &conn,
                &auth,
                "update",
                "item",
                Some(item_id),
                serde_json::json!({ "count": count }),
            )?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// 批量增量归类：按物化路径 ensure 祖先并关联（source='manual'，已有关联不动）。
/// 与 `set_item_categories`（整体替换）互补：批量条 / 多条目一次归入同一分类。
#[tauri::command]
pub fn add_item_categories(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    item_ids: Vec<i64>,
    path: String,
) -> Result<usize, String> {
    if item_ids.is_empty() {
        return Err("未选择条目".into());
    }
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("事务失败: {e}"))?;
    let inner = (|| -> Result<usize, String> {
        for id in &item_ids {
            let ex: Option<i64> = tx
                .query_row("SELECT 1 FROM items WHERE id = ?1", [id], |r| r.get(0))
                .optional()
                .map_err(|e| format!("查询条目失败: {e}"))?;
            if ex.is_none() {
                return Err(format!("条目不存在: {id}"));
            }
        }
        let cid = crate::categories::ensure_path(&tx, &path)?;
        let mut linked = 0usize;
        for id in &item_ids {
            linked += tx
                .execute(
                    "INSERT INTO item_categories (item_id, category_id, source)
                     VALUES (?1, ?2, 'manual')
                     ON CONFLICT DO NOTHING",
                    rusqlite::params![id, cid],
                )
                .map_err(|e| format!("关联分类失败: {e}"))?;
        }
        Ok(linked)
    })();
    match inner {
        Ok(linked) => {
            tx.commit().map_err(|e| format!("提交失败: {e}"))?;
            audit_now(
                &conn,
                &auth,
                "update",
                "item_categories",
                None,
                serde_json::json!({ "path": path, "items": item_ids.len(), "linked": linked }),
            )?;
            Ok(linked)
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------- 标签

#[tauri::command]
pub fn list_tags(state: State<'_, DbState>) -> Result<Vec<tags::TagRow>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    tags::list_tags(&conn)
}

#[tauri::command]
pub fn create_tag(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    name: String,
) -> Result<i64, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let id = tags::create_tag(&conn, &name)?;
    audit_now(
        &conn,
        &auth,
        "create",
        "tag",
        Some(id),
        serde_json::json!({ "name": name }),
    )?;
    Ok(id)
}

#[tauri::command]
pub fn rename_tag(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    tag_id: i64,
    name: String,
) -> Result<(), String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    tags::rename_tag(&conn, tag_id, &name)?;
    audit_now(
        &conn,
        &auth,
        "update",
        "tag",
        Some(tag_id),
        serde_json::json!({ "name": name }),
    )?;
    Ok(())
}

#[tauri::command]
pub fn add_tag_alias(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    alias: String,
    tag_id: i64,
) -> Result<(), String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    tags::add_alias(&conn, &alias, tag_id)?;
    audit_now(
        &conn,
        &auth,
        "create",
        "tag",
        Some(tag_id),
        serde_json::json!({ "alias": alias }),
    )?;
    Ok(())
}

#[tauri::command]
pub fn merge_tags(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    src_id: i64,
    dst_id: i64,
) -> Result<usize, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let n = tags::merge_tags(&mut conn, src_id, dst_id)?;
    // 被合并标签的条目全部重新索引
    let ids = affected_item_ids(&conn, src_id, dst_id)?;
    for id in ids {
        let _ = crate::fts::sync_item(&conn, id);
    }
    audit_now(
        &conn,
        &auth,
        "update",
        "tag",
        Some(dst_id),
        serde_json::json!({ "src_id": src_id, "moved": n }),
    )?;
    Ok(n)
}

fn affected_item_ids(
    conn: &rusqlite::Connection,
    src_id: i64,
    dst_id: i64,
) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT item_id FROM item_tags WHERE tag_id = ?1 OR tag_id = ?2")
        .map_err(|e| format!("查询受影响条目失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![src_id, dst_id], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("查询受影响条目失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("查询受影响条目失败: {e}"))?);
    }
    Ok(out)
}

/// 批量打标：item_ids × tag_names 增量写入，返回变更数并同步 FTS
#[tauri::command]
pub fn set_item_tags(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    item_ids: Vec<i64>,
    tag_names: Vec<String>,
) -> Result<usize, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let written = tags::add_tags_to_items(&mut conn, &item_ids, &tag_names)?;
    let mut synced = 0usize;
    for a in &written {
        if crate::fts::sync_item(&conn, a.item_id).is_ok() {
            synced += 1;
        }
    }
    audit_now(
        &conn,
        &auth,
        "update",
        "item",
        None,
        serde_json::json!({ "items": item_ids.len(), "tags": tag_names.len(), "written": written.len() }),
    )?;
    Ok(synced)
}

/// 批量移除标签
#[tauri::command]
pub fn remove_item_tags(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    item_ids: Vec<i64>,
    tag_ids: Vec<i64>,
) -> Result<usize, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let touched = tags::remove_tags_from_items(&conn, &item_ids, &tag_ids)?;
    let mut synced = 0usize;
    for id in &touched {
        if crate::fts::sync_item(&conn, *id).is_ok() {
            synced += 1;
        }
    }
    audit_now(
        &conn,
        &auth,
        "update",
        "item",
        None,
        serde_json::json!({ "items": item_ids.len(), "tags": tag_ids.len(), "touched": touched.len() }),
    )?;
    Ok(synced)
}

// ---------------------------------------------------------------- 智能集合

#[tauri::command]
pub fn list_collections(
    state: State<'_, DbState>,
) -> Result<Vec<crate::smart::CollectionRow>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    crate::smart::list_collections(&conn)
}

/// id 缺省 = 新建；query_json 保存前走白名单编译校验
#[tauri::command]
pub fn save_collection(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    id: Option<i64>,
    name: String,
    query_json: String,
    sort_order: Option<i64>,
) -> Result<i64, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let new_id =
        crate::smart::save_collection(&conn, id, &name, &query_json, sort_order.unwrap_or(0))?;
    let action = if id.is_none() { "create" } else { "update" };
    audit_now(
        &conn,
        &auth,
        action,
        "collection",
        Some(new_id),
        serde_json::json!({ "name": name }),
    )?;
    Ok(new_id)
}

#[tauri::command]
pub fn preview_collection(
    state: State<'_, DbState>,
    query_json: String,
    offset: Option<i64>,
) -> Result<(i64, Vec<browse::ItemRow>), String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    crate::smart::preview(&conn, &query_json, offset)
}

// ---------------------------------------------------------------- 规则自动打标

#[tauri::command]
pub fn list_rules(state: State<'_, DbState>) -> Result<Vec<crate::rules::RuleRow>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    crate::rules::list_rules(&conn)
}

/// id 缺省 = 新建；conditions/actions 保存前走白名单校验
#[tauri::command]
#[allow(clippy::too_many_arguments)] // 命令接口签名，参数结构随前端 invoke 约定
pub fn save_rule(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    id: Option<i64>,
    name: String,
    enabled: Option<bool>,
    priority: Option<i64>,
    conditions: String,
    actions: String,
) -> Result<i64, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let new_id = crate::rules::save_rule(
        &conn,
        id,
        &name,
        enabled.unwrap_or(true),
        priority.unwrap_or(0),
        &conditions,
        &actions,
    )?;
    let action = if id.is_none() { "create" } else { "update" };
    audit_now(
        &conn,
        &auth,
        action,
        "rule",
        Some(new_id),
        serde_json::json!({ "name": name }),
    )?;
    Ok(new_id)
}

#[tauri::command]
pub fn delete_rule(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    id: i64,
) -> Result<(), String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    crate::rules::delete_rule(&conn, id)?;
    audit_now(
        &conn,
        &auth,
        "delete",
        "rule",
        Some(id),
        serde_json::json!({}),
    )?;
    Ok(())
}

/// 后台跑全部启用规则（单事务），emit `rule-progress`；与扫描/哈希/元数据共用互斥
#[tauri::command(async)]
pub fn start_rule_job(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    root_id: Option<i64>,
) -> Result<(), String> {
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let mut conn = pool.get().map_err(|e| format!("数据库连接失败: {e}"))?;
            let mut emit = |p: &crate::rules::RuleProgress| {
                let _ = app.emit("rule-progress", p);
            };
            crate::rules::run_rules(&mut conn, root_id, &cancel, &mut emit).map(|_| ())
        }));
        let status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if result.is_err() || matches!(result.as_ref(), Ok(Err(_))) {
            "failed"
        } else {
            "done"
        };
        match result {
            Err(_) => {
                let _ = app2.emit(
                    "rule-progress",
                    crate::rules::RuleProgress::failure("规则线程崩溃"),
                );
            }
            Ok(Err(e)) => {
                let _ = app2.emit("rule-progress", crate::rules::RuleProgress::failure(e));
            }
            Ok(Ok(())) => {}
        }
        if let Ok(conn) = pool.get() {
            audit_job(
                &conn,
                audit_user,
                "rule",
                serde_json::json!({ "root_id": root_id, "status": status }),
            );
        }
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

/// 自动分类任务：目录结构镜像（settings `auto_cat_depth` 1..=5，缺省 1，0=关闭）
#[tauri::command(async)]
pub fn start_autocat_job(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    root_id: Option<i64>,
) -> Result<(), String> {
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let mut conn = pool.get().map_err(|e| format!("数据库连接失败: {e}"))?;
            let depth: usize = repo::get_settings(&conn)
                .ok()
                .and_then(|entries| {
                    entries
                        .into_iter()
                        .find(|e| e.key == "auto_cat_depth")
                        .map(|e| e.value)
                })
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(1)
                .min(5);
            let mut emit = |p: &crate::autocat::AutoCatProgress| {
                let _ = app.emit("autocat-progress", p);
            };
            crate::autocat::run(&mut conn, root_id, depth, &cancel, &mut emit).map(|_| ())
        }));
        let status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if result.is_err() || matches!(result.as_ref(), Ok(Err(_))) {
            "failed"
        } else {
            "done"
        };
        match result {
            Err(_) => {
                let _ = app2.emit(
                    "autocat-progress",
                    crate::autocat::AutoCatProgress {
                        phase: "finished".into(),
                        status: "error".into(),
                        message: "自动分类线程崩溃".into(),
                        stats: Default::default(),
                    },
                );
            }
            Ok(Err(e)) => {
                let _ = app2.emit(
                    "autocat-progress",
                    crate::autocat::AutoCatProgress {
                        phase: "finished".into(),
                        status: "error".into(),
                        message: e,
                        stats: Default::default(),
                    },
                );
            }
            Ok(Ok(())) => {}
        }
        if let Ok(conn) = pool.get() {
            audit_job(
                &conn,
                audit_user,
                "autocat",
                serde_json::json!({ "root_id": root_id, "status": status }),
            );
        }
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

fn tool_overrides(state: &DbState) -> HashMap<String, String> {
    let mut overrides = HashMap::new();
    if let Ok(conn) = state.pool.get() {
        if let Ok(entries) = repo::get_settings(&conn) {
            for e in entries {
                if e.key.ends_with("_path") && !e.value.trim().is_empty() {
                    overrides.insert(e.key, e.value);
                }
            }
        }
    }
    overrides
}

/// 元数据任务：source = "probe"（ffprobe/exiftool）| "beets" | "calibre"
#[tauri::command(async)]
pub fn start_meta_job(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    root_id: Option<i64>,
    source: String,
) -> Result<(), String> {
    use crate::meta::{beets, calibre, runner::ToolRunner, MetaProgress};

    if !matches!(source.as_str(), "probe" | "beets" | "calibre") {
        return Err(format!("未知元数据来源: {source}"));
    }
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let app2 = app.clone();
    let overrides = tool_overrides(&state);
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let mut conn = pool.get().map_err(|e| format!("数据库连接失败: {e}"))?;
            let mut emit = |p: &MetaProgress| {
                let _ = app.emit("meta-progress", p);
            };
            match source.as_str() {
                "probe" => {
                    let mut runner = ToolRunner::from_overrides(&overrides);
                    crate::meta::probe_root(&mut conn, root_id, &mut runner, &cancel, &mut emit)
                        .map(|_| ())
                }
                "beets" => {
                    let path = repo::get_setting_value(&conn, "beets_db_path")
                        .map_err(|e| format!("读取 beets_db_path 失败: {e}"))?;
                    if path.trim().is_empty() {
                        return Err("请先在设置中填写 beets library.db 路径".into());
                    }
                    beets::import(&mut conn, std::path::Path::new(&path), &cancel, &mut emit)
                        .map(|_| ())
                }
                _ => {
                    let path = repo::get_setting_value(&conn, "calibre_library_path")
                        .map_err(|e| format!("读取 calibre_library_path 失败: {e}"))?;
                    if path.trim().is_empty() {
                        return Err("请先在设置中填写 Calibre 书库路径".into());
                    }
                    calibre::import(&mut conn, std::path::Path::new(&path), &cancel, &mut emit)
                        .map(|_| ())
                }
            }
        }));
        let status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if result.is_err() || matches!(result.as_ref(), Ok(Err(_))) {
            "failed"
        } else {
            "done"
        };
        match result {
            Err(_) => {
                let _ = app2.emit(
                    "meta-progress",
                    crate::meta::MetaProgress::failure("元数据线程崩溃"),
                );
            }
            Ok(Err(e)) => {
                let _ = app2.emit("meta-progress", crate::meta::MetaProgress::failure(e));
            }
            Ok(Ok(())) => {}
        }
        if let Ok(conn) = pool.get() {
            audit_job(
                &conn,
                audit_user,
                "meta",
                serde_json::json!({ "root_id": root_id, "source": source, "status": status }),
            );
        }
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

// ---------------------------------------------------------------- 缩略图

/// 生成/返回缩略图绝对路径（已存在且新鲜则直接返回）。图片内部缩放，视频走 ffmpeg 抽帧。
#[tauri::command(async)]
pub fn ensure_thumb(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    item_id: i64,
) -> Result<crate::thumbs::ThumbResult, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let default_derived = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("无法解析 AppData 目录: {e}"))?
        .join("derived");
    let overrides = tool_overrides(&state);
    let ffmpeg = crate::thumbs::ffmpeg_program(&overrides);
    let r = crate::thumbs::ensure_thumb(&conn, item_id, &default_derived, &ffmpeg)?;
    if r.created {
        audit_now(
            &conn,
            &auth,
            "create",
            "thumb",
            Some(item_id),
            serde_json::json!({}),
        )?;
    }
    Ok(r)
}

// ---------------------------------------------------------------- 重复检测

/// 给 size 相同的可疑文件补部分哈希（只读原始文件，绝不删除/修改媒体）
#[tauri::command(async)]
pub fn start_dupe_job(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
) -> Result<(), String> {
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let conn = pool.get().map_err(|e| format!("数据库连接失败: {e}"))?;
            let roots: HashMap<i64, PathBuf> = repo::list_roots(&conn)
                .map_err(|e| format!("读取根目录失败: {e}"))?
                .into_iter()
                .map(|r| (r.id, PathBuf::from(r.path)))
                .collect();
            let mut emit = |p: &crate::dupes::DupeProgress| {
                let _ = app.emit("dupe-progress", p);
            };
            crate::dupes::run_partial_hashes(&conn, &roots, &cancel, &mut emit)
        }));
        let status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if result.is_err() || matches!(result.as_ref(), Ok(Err(_))) {
            "failed"
        } else {
            "done"
        };
        match result {
            Err(_) => {
                let _ = app2.emit(
                    "dupe-progress",
                    crate::dupes::DupeProgress::failure("重复检测线程崩溃"),
                );
            }
            Ok(Err(e)) => {
                let _ = app2.emit("dupe-progress", crate::dupes::DupeProgress::failure(e));
            }
            Ok(Ok(())) => {}
        }
        if let Ok(conn) = pool.get() {
            audit_job(
                &conn,
                audit_user,
                "dupe",
                serde_json::json!({ "status": status }),
            );
        }
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

/// 只读报告：部分哈希相同且 size 相同的组（组内按完整哈希切分/确认）
#[tauri::command]
pub fn list_dupes(state: State<'_, DbState>) -> Result<Vec<crate::dupes::DupeGroup>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    crate::dupes::report(&conn)
}

// ---------------------------------------------------------------- OCR 队列

/// OCR：book PDF → OCRmyPDF 派生副本（原始只读），文本入库 + FTS 重建
#[tauri::command(async)]
pub fn start_ocr_job(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    jobs: State<'_, JobState>,
    auth: State<'_, AuthState>,
    root_id: Option<i64>,
) -> Result<(), String> {
    let cancel = jobs.begin_job()?;
    let audit_user = auth.user_id();
    let pool = state.pool.clone();
    let overrides = tool_overrides(&state);
    let default_derived = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("无法定位 AppData 目录: {e}"))?
        .join("derived");
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let conn = pool.get().map_err(|e| format!("数据库连接失败: {e}"))?;
            let roots: HashMap<i64, PathBuf> = repo::list_roots(&conn)
                .map_err(|e| format!("读取根目录失败: {e}"))?
                .into_iter()
                .map(|r| (r.id, PathBuf::from(r.path)))
                .collect();
            let lang = repo::get_setting_value(&conn, "ocr_lang")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "eng".to_string());
            let runner = crate::ocr::OcrmypdfRunner::from_overrides(&overrides);
            let mut emit = |p: &crate::ocr::OcrProgress| {
                let _ = app.emit("ocr-progress", p);
            };
            crate::ocr::run_ocr_queue(
                &conn,
                &roots,
                &default_derived,
                &runner,
                &lang,
                root_id,
                &cancel,
                &mut emit,
            )
        }));
        let status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if result.is_err() || matches!(result.as_ref(), Ok(Err(_))) {
            "failed"
        } else {
            "done"
        };
        match result {
            Err(_) => {
                let _ = app2.emit(
                    "ocr-progress",
                    crate::ocr::OcrProgress::failure("OCR 线程崩溃"),
                );
            }
            Ok(Err(e)) => {
                let _ = app2.emit("ocr-progress", crate::ocr::OcrProgress::failure(e));
            }
            Ok(Ok(())) => {}
        }
        if let Ok(conn) = pool.get() {
            audit_job(
                &conn,
                audit_user,
                "ocr",
                serde_json::json!({ "status": status, "root_id": root_id }),
            );
        }
        if let Some(jobs) = app2.try_state::<JobState>() {
            jobs.end_job(&cancel);
        }
    });
    Ok(())
}

/// 待 OCR 数量（设置页面板徽标）
#[tauri::command]
pub fn ocr_pending_count(state: State<'_, DbState>, root_id: Option<i64>) -> Result<i64, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    crate::ocr::count_pending(&conn, root_id)
}

// ---------------------------------------------------------------- 备份

/// 备份：元数据快照（VACUUM INTO 在线一致副本）+ restic 集成钩子（可选、可降级）
#[tauri::command]
pub fn backup_now(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
) -> Result<crate::backup::BackupReport, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let overrides = tool_overrides(&state);
    let default_derived = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("无法定位 AppData 目录: {e}"))?
        .join("derived");
    let runner = crate::backup::CliRestic::from_overrides(&overrides);
    let report = crate::backup::run_backup(&mut conn, &default_derived, Some(&runner))?;
    audit_now(
        &conn,
        &auth,
        "create",
        "backup",
        None,
        serde_json::json!({
            "snapshot": report.snapshot_path,
            "kept": report.kept,
            "restic": report.restic.is_some(),
        }),
    )?;
    Ok(report)
}

// ---------------------------------------------------------------- 导入导出

/// 导出标签/分类 JSON 快照（写到指定路径）
#[tauri::command]
pub fn export_port(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    path: String,
) -> Result<usize, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let doc = crate::port::export_doc(&conn)?;
    let json = serde_json::to_string_pretty(&doc).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写入文件失败: {e}"))?;
    let n = doc.categories.len() + doc.tags.len();
    audit_now(
        &conn,
        &auth,
        "create",
        "export",
        None,
        serde_json::json!({
            "path": path,
            "categories": doc.categories.len(),
            "tags": doc.tags.len(),
        }),
    )?;
    Ok(n)
}

/// 从 JSON 文件导入标签/分类（增量 upsert，冲突项跳过并在报告列出）
#[tauri::command]
pub fn import_port(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    path: String,
) -> Result<crate::port::ImportReport, String> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let json = std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败: {e}"))?;
    let report = crate::port::import_json(&mut conn, &json)?;
    audit_now(
        &conn,
        &auth,
        "create",
        "import",
        None,
        serde_json::to_value(&report).map_err(|e| format!("序列化失败: {e}"))?,
    )?;
    Ok(report)
}

// ---------------------------------------------------------------- 用户 / 登录 / 审计

/// 首次启动创建初始管理员（仅 users 为空时可调），成功即登录
#[tauri::command]
pub fn setup_admin(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    username: String,
    password: String,
) -> Result<UserRow, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let id = auth::setup_admin(&conn, &username, &password)?;
    let user = auth::list_users(&conn)?
        .into_iter()
        .find(|u| u.id == id)
        .ok_or_else(|| "创建后读取用户失败".to_string())?;
    auth.login(user.clone());
    audit_now(
        &conn,
        &auth,
        "setup",
        "user",
        Some(id),
        serde_json::json!({ "username": user.username, "role": user.role }),
    )?;
    Ok(user)
}

#[tauri::command]
pub fn session_user(auth: State<'_, AuthState>) -> Option<UserRow> {
    auth.current()
}

#[tauri::command]
pub fn login(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    username: String,
    password: String,
) -> Result<UserRow, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    match auth::authenticate(&conn, &username, &password).map_err(|e| format!("登录失败: {e}"))?
    {
        Some(user) => {
            auth.login(user.clone());
            audit_now(
                &conn,
                &auth,
                "login",
                "session",
                Some(user.id),
                serde_json::json!({ "username": user.username }),
            )?;
            Ok(user)
        }
        None => {
            // 失败也记账（不记密码）
            audit::record(
                &conn,
                None,
                "login",
                "session",
                None,
                &serde_json::json!({ "ok": false, "username": username.trim() }).to_string(),
            )?;
            Err("用户名或密码错误".into())
        }
    }
}

#[tauri::command]
pub fn logout(state: State<'_, DbState>, auth: State<'_, AuthState>) -> Result<(), String> {
    if let Some(user) = auth.current() {
        let conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        audit_now(
            &conn,
            &auth,
            "logout",
            "session",
            Some(user.id),
            serde_json::json!({}),
        )?;
        auth.logout();
    }
    Ok(())
}

#[tauri::command]
pub fn change_password(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    let user = auth.current().ok_or_else(|| "未登录".to_string())?;
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    auth::change_password(&conn, user.id, &old_password, &new_password)?;
    audit_now(
        &conn,
        &auth,
        "password",
        "user",
        Some(user.id),
        serde_json::json!({}),
    )?;
    Ok(())
}

#[tauri::command]
pub fn list_users(state: State<'_, DbState>) -> Result<Vec<UserRow>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    auth::list_users(&conn)
}

#[tauri::command]
pub fn create_user(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    username: String,
    password: String,
    role: String,
) -> Result<i64, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    let id = auth::create_user(&conn, &username, &password, &role)?;
    audit_now(
        &conn,
        &auth,
        "create",
        "user",
        Some(id),
        serde_json::json!({ "username": username.trim(), "role": role }),
    )?;
    Ok(id)
}

#[tauri::command]
pub fn delete_user(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    user_id: i64,
) -> Result<(), String> {
    if auth.user_id() == Some(user_id) {
        return Err("不能删除当前登录用户".into());
    }
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    auth::delete_user(&conn, user_id)?;
    audit_now(
        &conn,
        &auth,
        "delete",
        "user",
        Some(user_id),
        serde_json::json!({}),
    )?;
    Ok(())
}

#[tauri::command]
pub fn set_user_role(
    state: State<'_, DbState>,
    auth: State<'_, AuthState>,
    user_id: i64,
    role: String,
) -> Result<(), String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    auth::set_role(&conn, user_id, &role)?;
    audit_now(
        &conn,
        &auth,
        "update",
        "user",
        Some(user_id),
        serde_json::json!({ "role": role }),
    )?;
    Ok(())
}

#[tauri::command]
pub fn list_audit(
    state: State<'_, DbState>,
    limit: Option<i64>,
    entity: Option<String>,
) -> Result<Vec<audit::AuditRow>, String> {
    let conn = state
        .pool
        .get()
        .map_err(|e| format!("数据库连接失败: {e}"))?;
    audit::recent(&conn, limit.unwrap_or(50), entity.as_deref())
}
