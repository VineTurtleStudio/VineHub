//! `--rpc` 模式：sidecar JSON-RPC over stdio（协议 v1，vineCurator）。
//!
//! - argv 最早分流，跳过 Tauri GUI / asset 协议作用域
//! - 方法面：`library.addRoot` / `library.roots` / `library.scan` /
//!   `search.items`（FTS5 优先、LIKE 兜底）/ `task.cancel`
//!   + P3.3 GUI 面覆盖（settings/roots/items/categories/tags/collections/rules/
//!   后台任务/缩略图/重复/OCR/备份/导入导出/用户/审计，见 handle_request）
//! - 打开与 GUI 同一 catalog.sqlite（WAL + busy_timeout，多进程可并存）
//! - 后台任务（扫描/哈希/元数据/规则/自动分类/重复/OCR）：ack `{taskId}` →
//!   `task.progress` 通知 → 同 id 终态响应；全局任务互斥（对应 GUI JobState），
//!   `task.cancel` 按 tasks 表取消

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use vine_plugin_api::{
    PluginInfo, RequestId, RpcError, RpcRequest, RpcResponse, ScanStartResult, TaskCancelParams,
    TaskProgressParams, codes, method, parse_request_line, to_line,
};

use crate::{
    audit, auth, autocat, backup, browse, categories, db, dupes, fts, meta, ocr, port, rules, scan,
    shell, smart, tags, tasks, thumbs,
};

const PLUGIN_ID: &str = "vine.curator";
const CAPABILITIES: &[&str] = &[
    "library", "search", "cancel", "browse", "categories", "tags", "collections", "rules", "jobs",
    "thumbs", "dupes", "ocr", "backup", "port", "auth", "settings", "shell",
];

struct TaskEntry {
    cancel: Arc<AtomicBool>,
}

struct RpcState {
    stdout: Mutex<std::io::Stdout>,
    tasks: Mutex<HashMap<String, TaskEntry>>,
    next_task: AtomicU64,
    pool: db::Pool,
    db_path: PathBuf,
    /// 全局任务互斥（对应 GUI `commands::JobState`：同一时刻最多一个后台任务）
    job: Mutex<Option<Arc<AtomicBool>>>,
    /// 进程内存登录会话（对应 GUI `auth::AuthState`）
    auth: auth::AuthState,
}

impl RpcState {
    fn send_line(&self, line: &str) {
        let mut out = self.stdout.lock().expect("stdout lock");
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    fn send_message(&self, message: &impl Serialize) {
        match to_line(message) {
            Ok(line) => self.send_line(&line),
            Err(e) => eprintln!("[vine-curator] 编码消息失败: {}", e.message),
        }
    }

    fn send_notification(&self, method_name: &str, params: impl Serialize) {
        let mut note = RpcRequest::notification(method_name);
        match serde_json::to_value(params) {
            Ok(v) => note.params = Some(v),
            Err(e) => {
                eprintln!("[vine-curator] 通知参数序列化失败: {e}");
                return;
            }
        }
        self.send_message(&note);
    }

    /// 开新任务：已有任务运行中（且未请求取消）返回 Err（poison 自行恢复）
    fn begin_job(&self) -> Result<Arc<AtomicBool>, String> {
        let mut slot = self.job.lock().unwrap_or_else(|p| p.into_inner());
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
        let mut slot = self.job.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(cur) = slot.as_ref() {
            if std::ptr::eq(cur.as_ref(), flag) {
                *slot = None;
            }
        }
    }

    /// 登记进 tasks 表（供 `task.cancel` / 退出等待）；并发上限 2
    fn register_task(&self, cancel: Arc<AtomicBool>) -> Result<String, RpcError> {
        let task_id = format!("t-{}", self.next_task.fetch_add(1, Ordering::SeqCst) + 1);
        let mut tasks = self.tasks.lock().expect("tasks lock");
        if tasks.len() >= 2 {
            return Err(RpcError::new(codes::PLUGIN_INTERNAL, "并发任务已达上限（2）"));
        }
        tasks.insert(task_id.clone(), TaskEntry { cancel });
        Ok(task_id)
    }

    /// 工具路径覆盖键（settings 中 `*_path` 非空项）
    fn tool_overrides(&self) -> HashMap<String, String> {
        let mut overrides = HashMap::new();
        if let Ok(conn) = self.pool.get() {
            if let Ok(entries) = db::repo::get_settings(&conn) {
                for e in entries {
                    if e.key.ends_with("_path") && !e.value.trim().is_empty() {
                        overrides.insert(e.key, e.value);
                    }
                }
            }
        }
        overrides
    }

    /// 写操作统一审计入口（对应 commands::audit_now；RPC 模式 user 取进程会话）
    fn audit_now(
        &self,
        conn: &rusqlite::Connection,
        action: &str,
        entity: &str,
        entity_id: Option<i64>,
        detail: Value,
    ) -> Result<(), String> {
        audit::record(
            conn,
            self.auth.user_id(),
            action,
            entity,
            entity_id.map(|v| v.to_string()).as_deref(),
            &detail.to_string(),
        )
    }
}

/// headless DB 路径：测试/嵌入可用 VINECURATOR_DB 覆盖，否则与 GUI 同路径。
fn headless_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("VINECURATOR_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let local = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    local.join("com.vineturtle.vinecurator").join("catalog.sqlite")
}

/// headless 派生数据默认目录（缩略图/OCR/备份回落；settings `derived_dir` 优先）
fn headless_derived_dir() -> PathBuf {
    let local = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    local.join("com.vineturtle.vinecurator").join("derived")
}

/// 与 GUI setup 等价的最小引导：开池 → 迁移 → FTS 一致性 → 种子集合。
fn bootstrap() -> Result<(db::Pool, PathBuf), String> {
    let path = headless_db_path();
    let pool = db::open_pool(&path).map_err(|e| format!("打开数据库失败: {e}"))?;
    {
        let mut conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
        let _ = db::migrate::migrate(&mut conn);
        fts::ensure_consistent(&mut conn).map_err(|e| format!("FTS 一致性检查失败: {e}"))?;
        smart::seed_example_collections(&conn).map_err(|e| format!("种子集合失败: {e}"))?;
    }
    Ok((pool, path))
}

/// 后台任务完成审计（线程内无 State，user 在启动时捕获）
fn audit_job(conn: &rusqlite::Connection, user: Option<i64>, kind: &str, detail: Value) {
    if let Err(e) = audit::record(conn, user, "run", "job", Some(kind), &detail.to_string()) {
        eprintln!("[vine-curator] 任务审计写入失败: {e}");
    }
}

/// `task.progress` 通知的 msg 组装：phase + （message 或 rel_path）
fn progress_msg(phase: &str, message: &str, rel_path: &str) -> Option<String> {
    let mut out = String::new();
    if !phase.is_empty() {
        out.push_str(phase);
    }
    let tail = if !message.is_empty() { message } else { rel_path };
    if !tail.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tail);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// RPC 模式主循环（stdin EOF 即退出）。
pub fn run() -> ExitCode {
    eprintln!(
        "[vine-curator] rpc mode started (api {}, pid {})",
        vine_plugin_api::HOST_API_VERSION,
        std::process::id()
    );

    let (pool, db_path) = match bootstrap() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[vine-curator] 数据库引导失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    let state = Arc::new(RpcState {
        stdout: Mutex::new(std::io::stdout()),
        tasks: Mutex::new(HashMap::new()),
        next_task: AtomicU64::new(0),
        pool,
        db_path,
        job: Mutex::new(None),
        auth: auth::AuthState::default(),
    });

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match parse_request_line(&line) {
            Ok(req) => handle_request(&state, req),
            Err(e) => {
                eprintln!("[vine-curator] 无法解析的请求行: {}", e.message);
                state.send_message(&RpcResponse::error(None, e.into_rpc_error()));
            }
        }
    }

    eprintln!("[vine-curator] rpc stdin closed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let active = state.tasks.lock().expect("tasks lock").len();
        if active == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("[vine-curator] 等待在途任务超时（{active} 个），强制退出");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    eprintln!("[vine-curator] rpc mode exit");
    ExitCode::SUCCESS
}

fn handle_request(state: &Arc<RpcState>, req: RpcRequest) {
    let Some(id) = req.id.clone() else {
        if req.method == method::TASK_CANCEL {
            match serde_json::from_value::<TaskCancelParams>(req.params.clone().unwrap_or(Value::Null))
            {
                Ok(params) => {
                    let tasks = state.tasks.lock().expect("tasks lock");
                    if let Some(task) = tasks.get(&params.task_id) {
                        task.cancel.store(true, Ordering::SeqCst);
                        eprintln!(
                            "[vine-curator] task {} cancel requested (notification)",
                            params.task_id
                        );
                    }
                }
                Err(e) => eprintln!("[vine-curator] task.cancel 参数非法: {e}"),
            }
        }
        return;
    };

    let response = match req.method.as_str() {
        method::PLUGIN_INFO => success(
            id,
            PluginInfo {
                id: PLUGIN_ID.into(),
                version: env!("CARGO_PKG_VERSION").into(),
                api_version: vine_plugin_api::HOST_API_VERSION.into(),
                capabilities: CAPABILITIES.iter().map(|s| s.to_string()).collect(),
            },
        ),
        method::PING => success(id, json!({ "pong": true })),
        // ---- 既有方法面 ----
        method::LIBRARY_ADD_ROOT => handle_add_root(state, id, req.params),
        method::LIBRARY_ROOTS => handle_roots(state, id),
        method::LIBRARY_SCAN => handle_library_scan(state, id, req.params),
        method::SEARCH_ITEMS => handle_search(state, id, req.params),
        // ---- P3.3 GUI 面覆盖 ----
        method::DB_INFO => handle_db_info(state, id),
        method::SETTINGS_GET => handle_settings_get(state, id),
        method::SETTINGS_SAVE => handle_settings_save(state, id, req.params),
        method::ROOTS_REMOVE => handle_roots_remove(state, id, req.params),
        method::ROOTS_REDIRECT => handle_roots_redirect(state, id, req.params),
        method::ITEMS_LIST => handle_items_list(state, id, req.params),
        method::OVERVIEW_GET => handle_overview(state, id, req.params),
        method::ITEM_GET => handle_item_get(state, id, req.params),
        method::ITEM_SET_FIELDS => handle_item_set_fields(state, id, req.params),
        method::PATH_OPEN => handle_path_shell(state, id, req.params, false),
        method::PATH_REVEAL => handle_path_shell(state, id, req.params, true),
        method::CATEGORIES_LIST => handle_categories_list(state, id),
        method::CATEGORY_CREATE => handle_category_create(state, id, req.params),
        method::CATEGORY_RENAME => handle_category_rename(state, id, req.params),
        method::CATEGORY_MOVE => handle_category_move(state, id, req.params),
        method::CATEGORY_DELETE => handle_category_delete(state, id, req.params),
        method::ITEM_CATEGORIES_SET => handle_item_categories_set(state, id, req.params),
        method::ITEM_CATEGORIES_ADD => handle_item_categories_add(state, id, req.params),
        method::TAGS_LIST => handle_tags_list(state, id),
        method::TAG_CREATE => handle_tag_create(state, id, req.params),
        method::TAG_RENAME => handle_tag_rename(state, id, req.params),
        method::TAG_ALIAS_ADD => handle_tag_alias_add(state, id, req.params),
        method::TAGS_MERGE => handle_tags_merge(state, id, req.params),
        method::ITEM_TAGS_SET => handle_item_tags_set(state, id, req.params),
        method::ITEM_TAGS_REMOVE => handle_item_tags_remove(state, id, req.params),
        method::COLLECTIONS_LIST => handle_collections_list(state, id),
        method::COLLECTION_SAVE => handle_collection_save(state, id, req.params),
        method::COLLECTION_PREVIEW => handle_collection_preview(state, id, req.params),
        method::RULES_LIST => handle_rules_list(state, id),
        method::RULE_SAVE => handle_rule_save(state, id, req.params),
        method::RULE_DELETE => handle_rule_delete(state, id, req.params),
        method::HASH_START => handle_hash_start(state, id, req.params),
        method::META_START => handle_meta_start(state, id, req.params),
        method::RULE_START => handle_rule_start(state, id, req.params),
        method::AUTOCAT_START => handle_autocat_start(state, id, req.params),
        method::DUPE_START => handle_dupe_start(state, id, req.params),
        method::OCR_START => handle_ocr_start(state, id, req.params),
        method::DUPES_LIST => handle_dupes_list(state, id),
        method::OCR_PENDING => handle_ocr_pending(state, id, req.params),
        method::THUMB_ENSURE => handle_thumb_ensure(state, id, req.params),
        method::BACKUP_NOW => handle_backup_now(state, id),
        method::PORT_EXPORT => handle_port_export(state, id, req.params),
        method::PORT_IMPORT => handle_port_import(state, id, req.params),
        method::ADMIN_SETUP => handle_admin_setup(state, id, req.params),
        method::SESSION_GET => success(id, state.auth.current()),
        method::AUTH_LOGIN => handle_auth_login(state, id, req.params),
        method::AUTH_LOGOUT => handle_auth_logout(state, id),
        method::AUTH_PASSWORD_CHANGE => handle_password_change(state, id, req.params),
        method::USERS_LIST => handle_users_list(state, id),
        method::USER_CREATE => handle_user_create(state, id, req.params),
        method::USER_DELETE => handle_user_delete(state, id, req.params),
        method::USER_ROLE_SET => handle_user_role_set(state, id, req.params),
        method::AUDIT_LIST => handle_audit_list(state, id, req.params),
        method::TASK_CANCEL => handle_task_cancel(state, id, req.params),
        other => RpcResponse::error(
            Some(id),
            RpcError::new(codes::METHOD_NOT_FOUND, format!("方法不存在: {other}")),
        ),
    };
    state.send_message(&response);
}

fn success(id: RequestId, result: impl Serialize) -> RpcResponse {
    match RpcResponse::success(id, result) {
        Ok(r) => r,
        Err(e) => RpcResponse::error(None, e.into_rpc_error()),
    }
}

fn invalid_params(id: RequestId, message: impl Into<String>) -> RpcResponse {
    RpcResponse::error(Some(id), RpcError::new(codes::INVALID_PARAMS, message.into()))
}

fn internal(id: RequestId, message: impl Into<String>) -> RpcResponse {
    RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, message.into()))
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Option<Value>) -> Result<T, String> {
    serde_json::from_value(params.unwrap_or_else(|| json!({}))).map_err(|e| format!("参数非法: {e}"))
}

// ---------------------------------------------------------------- 序列化辅助

fn root_json(r: &db::repo::RootEntry) -> Value {
    json!({
        "id": r.id,
        "path": r.path,
        "label": r.label,
        "itemCount": r.item_count,
        "createdAt": r.created_at,
    })
}

fn item_json(i: &browse::ItemRow) -> Value {
    json!({
        "id": i.id,
        "rootId": i.root_id,
        "relPath": i.rel_path,
        "fileName": i.file_name,
        "ext": i.ext,
        "typeName": i.type_name,
        "size": i.size,
        "mtime": i.mtime,
        "status": i.status,
        "rating": i.rating,
        "year": i.year,
        "missing": i.missing,
        "hashState": i.hash_state,
        "metaState": i.meta_state,
    })
}

// ---------------------------------------------------------------- 后台任务框架

/// 启动后台任务：ack `{taskId}` 立即返回，线程内跑 `run` 并发终态（同 id）。
/// `run` 返回 (status, stats)；status = done|cancelled|failed。
fn start_job(
    state: &Arc<RpcState>,
    id: RequestId,
    kind: &'static str,
    run: impl FnOnce(Arc<RpcState>, String, Arc<AtomicBool>) -> Result<(String, Value), String>
    + Send
    + 'static,
) -> RpcResponse {
    let cancel = match state.begin_job() {
        Ok(f) => f,
        Err(e) => return internal(id, e),
    };
    let task_id = match state.register_task(cancel.clone()) {
        Ok(t) => t,
        Err(e) => {
            state.end_job(&cancel);
            return RpcResponse::error(Some(id), e);
        }
    };
    let state2 = state.clone();
    let task_id2 = task_id.clone();
    let user = state.auth.user_id();
    let id_for_thread = id.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let kind_owned = kind;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(state2.clone(), task_id2.clone(), cancel.clone())
        }));
        let elapsed = start.elapsed().as_millis() as u64;
        let cancelled = cancel.load(Ordering::Relaxed);
        let (status, stats) = match outcome {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                eprintln!("[vine-curator] 任务 {kind} 失败: {e}");
                ("failed".to_string(), json!({ "error": e }))
            }
            Err(_) => {
                eprintln!("[vine-curator] 任务 {kind} 线程崩溃");
                ("failed".to_string(), json!({ "error": "线程崩溃" }))
            }
        };
        if let Ok(conn) = state2.pool.get() {
            audit_job(
                &conn,
                user,
                kind_owned,
                json!({ "taskId": task_id2, "status": status, "elapsedMs": elapsed }),
            );
        }
        let terminal = json!({
            "taskId": task_id2,
            "status": if cancelled && status != "failed" {
                "cancelled".to_string()
            } else {
                status
            },
            "stats": stats,
            "cancelled": cancelled,
            "elapsedMs": elapsed,
        });
        state2.send_message(&success(id_for_thread, terminal));
        state2.tasks.lock().expect("tasks lock").remove(&task_id2);
        state2.end_job(&cancel);
    });
    success(id, ScanStartResult { task_id })
}

// ---------------------------------------------------------------- 基础：library / search / task

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddRootParams {
    path: String,
    #[serde(default)]
    label: Option<String>,
}

fn handle_add_root(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: AddRootParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let label = p.label.unwrap_or_default();
    match db::repo::add_root(&mut conn, &p.path, &label) {
        Ok(root) => {
            let _ = state.audit_now(
                &conn,
                "create",
                "root",
                Some(root.id),
                json!({ "path": p.path }),
            );
            success(id, root_json(&root))
        }
        Err(e) => invalid_params(id, e),
    }
}

fn handle_roots(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match db::repo::list_roots(&conn) {
        Ok(roots) => {
            let arr: Vec<Value> = roots.iter().map(root_json).collect();
            success(id, json!({ "roots": arr }))
        }
        Err(e) => internal(id, format!("读取根目录失败: {e}")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanParams {
    root_id: i64,
}

fn handle_library_scan(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: ScanParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    {
        let conn = match state.pool.get() {
            Ok(c) => c,
            Err(e) => return internal(id, format!("数据库连接失败: {e}")),
        };
        if db::repo::get_root_path(&conn, p.root_id).is_err() {
            return invalid_params(id, format!("根目录不存在: {}", p.root_id));
        }
    }
    start_job(state, id, "scan", move |state, task_id, cancel| {
        let mut conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let root_path = PathBuf::from(
            db::repo::get_root_path(&conn, p.root_id).map_err(|e| e)?,
        );
        let mut emit = |pr: &scan::ScanProgress| {
            let done = pr.index as u64;
            let total = pr.total as u64;
            let msg = progress_msg(&pr.phase, &pr.message, &pr.rel_path);
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done,
                    total,
                    msg,
                },
            );
        };
        let outcome =
            scan::scan_root(&mut conn, p.root_id, &root_path, &cancel, &mut emit)?;
        Ok((
            outcome.status,
            serde_json::to_value(&outcome.stats)
                .map_err(|e| format!("序列化统计失败: {e}"))?,
        ))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    query: String,
    #[serde(default)]
    limit: Option<i64>,
}

fn handle_search(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: SearchParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    if p.query.trim().is_empty() {
        return invalid_params(id, "query 不能为空");
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match browse::search_items(&conn, &p.query, p.limit) {
        Ok(items) => {
            let total = items.len() as i64;
            let arr: Vec<Value> = items.iter().map(item_json).collect();
            success(id, json!({ "total": total, "items": arr }))
        }
        Err(e) => internal(id, e),
    }
}

fn handle_task_cancel(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: TaskCancelParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let found = {
        let tasks = state.tasks.lock().expect("tasks lock");
        match tasks.get(&p.task_id) {
            Some(t) => {
                t.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    };
    if found {
        if let Ok(conn) = state.pool.get() {
            let _ = state.audit_now(&conn, "cancel", "job", None, json!({ "taskId": p.task_id }));
        }
        success(id, json!({ "cancelled": true }))
    } else {
        RpcResponse::error(
            Some(id),
            RpcError::new(codes::TASK_NOT_FOUND, format!("任务不存在: {}", p.task_id)),
        )
    }
}

// ---------------------------------------------------------------- 设置 / 根目录

fn handle_db_info(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    success(
        id,
        json!({ "path": state.db_path.to_string_lossy() }),
    )
}

fn handle_settings_get(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match db::repo::get_settings(&conn) {
        Ok(entries) => success(id, entries),
        Err(e) => internal(id, format!("读取设置失败: {e}")),
    }
}

#[derive(Deserialize)]
struct SettingsSaveParams {
    entries: Vec<db::repo::SettingEntry>,
}

fn handle_settings_save(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: SettingsSaveParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    if p.entries.is_empty() {
        return success(id, json!({}));
    }
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match db::repo::save_settings(&mut conn, &p.entries) {
        Ok(()) => {
            let keys: Vec<&str> = p.entries.iter().map(|e| e.key.as_str()).collect();
            let _ = state.audit_now(
                &conn,
                "update",
                "settings",
                None,
                json!({ "keys": keys }),
            );
            success(id, json!({}))
        }
        Err(e) => internal(id, format!("保存设置失败: {e}")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootsRemoveParams {
    root_id: i64,
}

fn handle_roots_remove(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: RootsRemoveParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match db::repo::remove_root(&mut conn, p.root_id) {
        Ok(()) => {
            let _ = fts::prune_orphans(&conn);
            let _ = state.audit_now(&conn, "delete", "root", Some(p.root_id), json!({}));
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootsRedirectParams {
    root_id: i64,
    new_path: String,
}

fn handle_roots_redirect(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: RootsRedirectParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match db::repo::redirect_root(&mut conn, p.root_id, &p.new_path) {
        Ok(root) => {
            let _ = state.audit_now(
                &conn,
                "update",
                "root",
                Some(p.root_id),
                json!({ "new_path": p.new_path }),
            );
            success(id, root_json(&root))
        }
        Err(e) => invalid_params(id, e),
    }
}

// ---------------------------------------------------------------- 浏览 / 条目

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemsListParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
    #[serde(default, alias = "type_name")]
    type_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, alias = "category_id")]
    category_id: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    desc: Option<bool>,
}

fn handle_items_list(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: ItemsListParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let filter = browse::ItemFilter {
        root_id: p.root_id,
        type_name: p.type_name,
        status: p.status,
        category_id: p.category_id,
        offset: p.offset,
        limit: p.limit,
        sort: p.sort,
        desc: p.desc,
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match browse::list_items(&conn, &filter) {
        Ok(page) => success(
            id,
            browse::ItemPage {
                total: page.total,
                items: page.items,
            },
        ),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverviewParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
}

fn handle_overview(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: OverviewParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match browse::overview(&conn, p.root_id) {
        Ok(o) => success(id, o),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemGetParams {
    item_id: i64,
}

fn handle_item_get(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: ItemGetParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match browse::get_item(&conn, p.item_id) {
        Ok(detail) => success(id, detail),
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemSetFieldsParams {
    item_ids: Vec<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    rating: Option<i64>,
    #[serde(default)]
    year: Option<i64>,
}

fn handle_item_set_fields(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: ItemSetFieldsParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    if p.status.is_none() && p.rating.is_none() && p.year.is_none() {
        return invalid_params(id, "没有要修改的字段");
    }
    if let Some(s) = &p.status {
        const ALLOWED: [&str; 5] = ["inbox", "unwatched", "reading", "done", "dropped"];
        if !ALLOWED.contains(&s.as_str()) {
            return invalid_params(id, format!("非法状态: {s}"));
        }
    }
    if let Some(r) = p.rating {
        if !(0..=5).contains(&r) {
            return invalid_params(id, format!("评分须在 0..=5: {r}"));
        }
    }
    let mut sets: Vec<&str> = Vec::new();
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(s) = &p.status {
        sets.push("status = ?1");
        args.push(s.clone().into());
    }
    if let Some(r) = p.rating {
        sets.push(if args.is_empty() { "?1" } else { "?2" });
        args.push(r.into());
    }
    if let Some(y) = p.year {
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
    let ids_json = match serde_json::to_string(&p.item_ids) {
        Ok(j) => j,
        Err(e) => return internal(id, format!("序列化失败: {e}")),
    };
    args.push(ids_json.into());
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let n = match conn.execute(&sql, rusqlite::params_from_iter(args.iter())) {
        Ok(n) => n,
        Err(e) => return internal(id, format!("更新失败: {e}")),
    };
    for item_id in &p.item_ids {
        let _ = fts::sync_item(&conn, *item_id);
    }
    let _ = state.audit_now(
        &conn,
        "update",
        "item",
        None,
        json!({ "count": n, "status": p.status, "rating": p.rating, "year": p.year }),
    );
    success(id, json!({ "updated": n }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathParams {
    path: String,
}

fn handle_path_shell(
    _state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
    reveal: bool,
) -> RpcResponse {
    let p: PathParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let r = if reveal {
        shell::reveal_path(&p.path)
    } else {
        shell::open_path(&p.path)
    };
    match r {
        Ok(()) => success(id, json!({})),
        Err(e) => internal(id, e),
    }
}

// ---------------------------------------------------------------- 分类树

fn handle_categories_list(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match browse::list_categories(&conn) {
        Ok(nodes) => success(id, nodes),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CategoryCreateParams {
    #[serde(default, alias = "parent_id")]
    parent_id: Option<i64>,
    name: String,
}

fn handle_category_create(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: CategoryCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match categories::create_category(&mut conn, p.parent_id, &p.name) {
        Ok(cid) => {
            let _ = state.audit_now(&conn, "create", "category", Some(cid), json!({ "name": p.name }));
            success(id, json!({ "id": cid }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CategoryRenameParams {
    category_id: i64,
    name: String,
}

fn handle_category_rename(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: CategoryRenameParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match categories::rename_category(&mut conn, p.category_id, &p.name) {
        Ok(()) => {
            let _ = state.audit_now(
                &conn,
                "update",
                "category",
                Some(p.category_id),
                json!({ "name": p.name }),
            );
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CategoryMoveParams {
    category_id: i64,
    #[serde(default, alias = "parent_id")]
    parent_id: Option<i64>,
}

fn handle_category_move(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: CategoryMoveParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match categories::move_category(&mut conn, p.category_id, p.parent_id) {
        Ok(()) => {
            let _ = state.audit_now(
                &conn,
                "update",
                "category",
                Some(p.category_id),
                json!({ "parent_id": p.parent_id }),
            );
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CategoryDeleteParams {
    category_id: i64,
}

fn handle_category_delete(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: CategoryDeleteParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match categories::delete_category(&conn, p.category_id) {
        Ok(_) => {
            let _ = state.audit_now(&conn, "delete", "category", Some(p.category_id), json!({}));
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemCategoriesSetParams {
    item_id: i64,
    #[serde(alias = "category_ids")]
    category_ids: Vec<i64>,
}

fn handle_item_categories_set(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: ItemCategoriesSetParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let count = p.category_ids.len();
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => return internal(id, format!("事务失败: {e}")),
    };
    let inner = (|| -> Result<(), String> {
        use rusqlite::OptionalExtension;
        let exists: Option<i64> = tx
            .query_row("SELECT 1 FROM items WHERE id = ?1", [p.item_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("查询条目失败: {e}"))?;
        if exists.is_none() {
            return Err(format!("条目不存在: {}", p.item_id));
        }
        tx.execute("DELETE FROM item_categories WHERE item_id = ?1", [p.item_id])
            .map_err(|e| format!("清除分类关联失败: {e}"))?;
        for cid in &p.category_ids {
            tx.execute(
                "INSERT INTO item_categories (item_id, category_id, source) VALUES (?1, ?2, 'manual')
                 ON CONFLICT DO NOTHING",
                rusqlite::params![p.item_id, cid],
            )
            .map_err(|e| format!("关联分类失败: {e}"))?;
        }
        Ok(())
    })();
    match inner {
        Ok(()) => {
            if let Err(e) = tx.commit() {
                return internal(id, format!("提交失败: {e}"));
            }
            let _ = state.audit_now(
                &conn,
                "update",
                "item",
                Some(p.item_id),
                json!({ "count": count }),
            );
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemCategoriesAddParams {
    #[serde(alias = "item_ids")]
    item_ids: Vec<i64>,
    path: String,
}

fn handle_item_categories_add(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: ItemCategoriesAddParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    if p.item_ids.is_empty() {
        return invalid_params(id, "未选择条目");
    }
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => return internal(id, format!("事务失败: {e}")),
    };
    let inner = (|| -> Result<usize, String> {
        use rusqlite::OptionalExtension;
        for item_id in &p.item_ids {
            let ex: Option<i64> = tx
                .query_row("SELECT 1 FROM items WHERE id = ?1", [item_id], |r| r.get(0))
                .optional()
                .map_err(|e| format!("查询条目失败: {e}"))?;
            if ex.is_none() {
                return Err(format!("条目不存在: {item_id}"));
            }
        }
        let cid = categories::ensure_path(&tx, &p.path)?;
        let mut linked = 0usize;
        for item_id in &p.item_ids {
            linked += tx
                .execute(
                    "INSERT INTO item_categories (item_id, category_id, source)
                     VALUES (?1, ?2, 'manual')
                     ON CONFLICT DO NOTHING",
                    rusqlite::params![item_id, cid],
                )
                .map_err(|e| format!("关联分类失败: {e}"))?;
        }
        Ok(linked)
    })();
    match inner {
        Ok(linked) => {
            if let Err(e) = tx.commit() {
                return internal(id, format!("提交失败: {e}"));
            }
            let _ = state.audit_now(
                &conn,
                "update",
                "item_categories",
                None,
                json!({ "path": p.path, "items": p.item_ids.len(), "linked": linked }),
            );
            success(id, json!({ "linked": linked }))
        }
        Err(e) => invalid_params(id, e),
    }
}

// ---------------------------------------------------------------- 标签

fn handle_tags_list(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::list_tags(&conn) {
        Ok(rows) => success(id, rows),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagCreateParams {
    name: String,
}

fn handle_tag_create(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: TagCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::create_tag(&conn, &p.name) {
        Ok(tid) => {
            let _ = state.audit_now(&conn, "create", "tag", Some(tid), json!({ "name": p.name }));
            success(id, json!({ "id": tid }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagRenameParams {
    tag_id: i64,
    name: String,
}

fn handle_tag_rename(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: TagRenameParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::rename_tag(&conn, p.tag_id, &p.name) {
        Ok(()) => {
            let _ = state.audit_now(
                &conn,
                "update",
                "tag",
                Some(p.tag_id),
                json!({ "name": p.name }),
            );
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagAliasParams {
    alias: String,
    tag_id: i64,
}

fn handle_tag_alias_add(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: TagAliasParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::add_alias(&conn, &p.alias, p.tag_id) {
        Ok(()) => {
            let _ = state.audit_now(
                &conn,
                "create",
                "tag",
                Some(p.tag_id),
                json!({ "alias": p.alias }),
            );
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagsMergeParams {
    src_id: i64,
    dst_id: i64,
}

fn handle_tags_merge(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: TagsMergeParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::merge_tags(&mut conn, p.src_id, p.dst_id) {
        Ok(n) => {
            let mut stmt = match conn
                .prepare("SELECT DISTINCT item_id FROM item_tags WHERE tag_id = ?1 OR tag_id = ?2")
            {
                Ok(s) => s,
                Err(e) => return internal(id, format!("查询受影响条目失败: {e}")),
            };
            let ids: Vec<i64> = match stmt
                .query_map(rusqlite::params![p.src_id, p.dst_id], |r| r.get::<_, i64>(0))
            {
                Ok(rows) => rows.filter_map(Result::ok).collect(),
                Err(e) => return internal(id, format!("查询受影响条目失败: {e}")),
            };
            for item_id in ids {
                let _ = fts::sync_item(&conn, item_id);
            }
            let _ = state.audit_now(
                &conn,
                "update",
                "tag",
                Some(p.dst_id),
                json!({ "src_id": p.src_id, "moved": n }),
            );
            success(id, json!({ "moved": n }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemTagsSetParams {
    #[serde(alias = "item_ids")]
    item_ids: Vec<i64>,
    #[serde(alias = "tag_names")]
    tag_names: Vec<String>,
}

fn handle_item_tags_set(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: ItemTagsSetParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::add_tags_to_items(&mut conn, &p.item_ids, &p.tag_names) {
        Ok(written) => {
            let mut synced = 0usize;
            for a in &written {
                if fts::sync_item(&conn, a.item_id).is_ok() {
                    synced += 1;
                }
            }
            let _ = state.audit_now(
                &conn,
                "update",
                "item",
                None,
                json!({ "items": p.item_ids.len(), "tags": p.tag_names.len(), "written": written.len() }),
            );
            success(id, json!({ "synced": synced }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemTagsRemoveParams {
    #[serde(alias = "item_ids")]
    item_ids: Vec<i64>,
    #[serde(alias = "tag_ids")]
    tag_ids: Vec<i64>,
}

fn handle_item_tags_remove(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: ItemTagsRemoveParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match tags::remove_tags_from_items(&conn, &p.item_ids, &p.tag_ids) {
        Ok(touched) => {
            let mut synced = 0usize;
            for item_id in &touched {
                if fts::sync_item(&conn, *item_id).is_ok() {
                    synced += 1;
                }
            }
            let _ = state.audit_now(
                &conn,
                "update",
                "item",
                None,
                json!({ "items": p.item_ids.len(), "tags": p.tag_ids.len(), "touched": touched.len() }),
            );
            success(id, json!({ "synced": synced }))
        }
        Err(e) => invalid_params(id, e),
    }
}

// ---------------------------------------------------------------- 智能集合

fn handle_collections_list(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match smart::list_collections(&conn) {
        Ok(rows) => success(id, rows),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionSaveParams {
    #[serde(default)]
    id: Option<i64>,
    name: String,
    query_json: String,
    #[serde(default, alias = "sort_order")]
    sort_order: Option<i64>,
}

fn handle_collection_save(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: CollectionSaveParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match smart::save_collection(&conn, p.id, &p.name, &p.query_json, p.sort_order.unwrap_or(0)) {
        Ok(cid) => {
            let action = if p.id.is_none() { "create" } else { "update" };
            let _ = state.audit_now(
                &conn,
                action,
                "collection",
                Some(cid),
                json!({ "name": p.name }),
            );
            success(id, json!({ "id": cid }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionPreviewParams {
    query_json: String,
    #[serde(default)]
    offset: Option<i64>,
}

fn handle_collection_preview(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: CollectionPreviewParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match smart::preview(&conn, &p.query_json, p.offset) {
        Ok((total, items)) => success(
            id,
            browse::ItemPage {
                total,
                items,
            },
        ),
        Err(e) => invalid_params(id, e),
    }
}

// ---------------------------------------------------------------- 规则 CRUD

fn handle_rules_list(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match rules::list_rules(&conn) {
        Ok(rows) => success(id, rows),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleSaveParams {
    #[serde(default)]
    id: Option<i64>,
    name: String,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    priority: Option<i64>,
    conditions: String,
    actions: String,
}

fn handle_rule_save(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: RuleSaveParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match rules::save_rule(
        &conn,
        p.id,
        &p.name,
        p.enabled.unwrap_or(true),
        p.priority.unwrap_or(0),
        &p.conditions,
        &p.actions,
    ) {
        Ok(rid) => {
            let action = if p.id.is_none() { "create" } else { "update" };
            let _ = state.audit_now(&conn, action, "rule", Some(rid), json!({ "name": p.name }));
            success(id, json!({ "id": rid }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleDeleteParams {
    id: i64,
}

fn handle_rule_delete(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: RuleDeleteParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match rules::delete_rule(&conn, p.id) {
        Ok(()) => {
            let _ = state.audit_now(&conn, "delete", "rule", Some(p.id), json!({}));
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

// ---------------------------------------------------------------- 后台任务

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HashStartParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
}

fn handle_hash_start(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: HashStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    start_job(state, id, "hash", move |state, task_id, cancel| {
        let conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let roots: Vec<(i64, PathBuf)> = match p.root_id {
            Some(rid) => {
                let path = db::repo::get_root_path(&conn, rid)?;
                vec![(rid, PathBuf::from(path))]
            }
            None => db::repo::list_roots(&conn)
                .map_err(|e| format!("读取根目录失败: {e}"))?
                .into_iter()
                .map(|r| (r.id, PathBuf::from(r.path)))
                .collect(),
        };
        for (rid, _) in &roots {
            tasks::mark_pending(&conn, Some(*rid), &[])?;
        }
        let mut last_done = 0u64;
        let mut last_failed = 0u64;
        let mut emit = |pr: &tasks::HashProgress| {
            last_done = pr.stats.done;
            last_failed = pr.stats.failed;
            let msg = progress_msg(&pr.phase, &pr.message, &pr.rel_path);
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done: pr.stats.done,
                    total: pr.stats.done + pr.stats.failed,
                    msg,
                },
            );
        };
        let mut status = "done".to_string();
        for (_, root_path) in &roots {
            if cancel.load(Ordering::Relaxed) {
                status = "cancelled".into();
                break;
            }
            match tasks::run_pending_hashes(&conn, root_path, &cancel, &mut emit) {
                Ok(outcome) => {
                    if outcome.status != "done" {
                        status = outcome.status;
                    }
                }
                Err(e) => {
                    drop(emit);
                    return Err(e);
                }
            }
        }
        drop(emit);
        Ok((
            status,
            json!({ "done": last_done, "failed": last_failed }),
        ))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetaStartParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
    source: String,
}

fn handle_meta_start(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: MetaStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    if !matches!(p.source.as_str(), "probe" | "beets" | "calibre") {
        return invalid_params(id, format!("未知元数据来源: {}", p.source));
    }
    start_job(state, id, "meta", move |state, task_id, cancel| {
        use crate::meta::{beets, calibre, runner::ToolRunner, MetaProgress};
        let mut conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let overrides = state.tool_overrides();
        let mut emit = |pr: &MetaProgress| {
            let msg = progress_msg(&pr.phase, &pr.message, &pr.rel_path);
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done: pr.stats.probed + pr.stats.skipped + pr.stats.failed,
                    total: pr.total as u64,
                    msg,
                },
            );
        };
        let outcome = match p.source.as_str() {
            "probe" => {
                let mut runner = ToolRunner::from_overrides(&overrides);
                meta::probe_root(&mut conn, p.root_id, &mut runner, &cancel, &mut emit)?
            }
            "beets" => {
                let path = db::repo::get_setting_value(&conn, "beets_db_path")
                    .map_err(|e| format!("读取 beets_db_path 失败: {e}"))?;
                if path.trim().is_empty() {
                    return Err("请先在设置中填写 beets library.db 路径".into());
                }
                beets::import(&mut conn, std::path::Path::new(&path), &cancel, &mut emit)?
            }
            _ => {
                let path = db::repo::get_setting_value(&conn, "calibre_library_path")
                    .map_err(|e| format!("读取 calibre_library_path 失败: {e}"))?;
                if path.trim().is_empty() {
                    return Err("请先在设置中填写 Calibre 书库路径".into());
                }
                calibre::import(&mut conn, std::path::Path::new(&path), &cancel, &mut emit)?
            }
        };
        Ok((
            outcome.status,
            serde_json::to_value(&outcome.stats)
                .map_err(|e| format!("序列化统计失败: {e}"))?,
        ))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleStartParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
}

fn handle_rule_start(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: RuleStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    start_job(state, id, "rule", move |state, task_id, cancel| {
        let mut conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let mut emit = |pr: &rules::RuleProgress| {
            let msg = progress_msg(&pr.phase, &pr.message, &pr.rule_name);
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done: pr.index as u64,
                    total: pr.total as u64,
                    msg,
                },
            );
        };
        let stats = rules::run_rules(&mut conn, p.root_id, &cancel, &mut emit)?;
        Ok((
            "done".to_string(),
            serde_json::to_value(&stats).map_err(|e| format!("序列化统计失败: {e}"))?,
        ))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutocatStartParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
}

fn handle_autocat_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: AutocatStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    start_job(state, id, "autocat", move |state, task_id, cancel| {
        let mut conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let depth: usize = db::repo::get_settings(&conn)
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
        let mut emit = |pr: &autocat::AutoCatProgress| {
            let msg = progress_msg(&pr.phase, &pr.message, "");
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done: pr.stats.items as u64,
                    total: 0,
                    msg,
                },
            );
        };
        let stats = autocat::run(&mut conn, p.root_id, depth, &cancel, &mut emit)?;
        Ok((
            "done".to_string(),
            serde_json::to_value(&stats).map_err(|e| format!("序列化统计失败: {e}"))?,
        ))
    })
}

fn handle_dupe_start(state: &Arc<RpcState>, id: RequestId, _params: Option<Value>) -> RpcResponse {
    start_job(state, id, "dupe", move |state, task_id, cancel| {
        let conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let roots: HashMap<i64, PathBuf> = db::repo::list_roots(&conn)
            .map_err(|e| format!("读取根目录失败: {e}"))?
            .into_iter()
            .map(|r| (r.id, PathBuf::from(r.path)))
            .collect();
        let mut last_hashed = 0usize;
        let mut last_failed = 0usize;
        let mut emit = |pr: &dupes::DupeProgress| {
            last_hashed = pr.hashed;
            last_failed = pr.failed;
            let msg = progress_msg(&pr.phase, &pr.message, &pr.rel_path);
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done: pr.hashed as u64,
                    total: pr.total as u64,
                    msg,
                },
            );
        };
        if let Err(e) = dupes::run_partial_hashes(&conn, &roots, &cancel, &mut emit) {
            drop(emit);
            return Err(e);
        }
        drop(emit);
        Ok((
            if cancel.load(Ordering::Relaxed) {
                "cancelled".to_string()
            } else {
                "done".to_string()
            },
            json!({ "hashed": last_hashed, "failed": last_failed }),
        ))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OcrStartParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
}

fn handle_ocr_start(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: OcrStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    start_job(state, id, "ocr", move |state, task_id, cancel| {
        let conn = state
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {e}"))?;
        let roots: HashMap<i64, PathBuf> = db::repo::list_roots(&conn)
            .map_err(|e| format!("读取根目录失败: {e}"))?
            .into_iter()
            .map(|r| (r.id, PathBuf::from(r.path)))
            .collect();
        let lang = db::repo::get_setting_value(&conn, "ocr_lang")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "eng".to_string());
        let overrides = state.tool_overrides();
        let runner = ocr::OcrmypdfRunner::from_overrides(&overrides);
        let default_derived = headless_derived_dir();
        let mut last_done = 0usize;
        let mut last_failed = 0usize;
        let mut emit = |pr: &ocr::OcrProgress| {
            last_done = pr.done;
            last_failed = pr.failed;
            let msg = progress_msg(&pr.phase, &pr.message, &pr.rel_path);
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_id.clone(),
                    done: pr.done as u64,
                    total: pr.total as u64,
                    msg,
                },
            );
        };
        if let Err(e) = ocr::run_ocr_queue(
            &conn,
            &roots,
            &default_derived,
            &runner,
            &lang,
            p.root_id,
            &cancel,
            &mut emit,
        ) {
            drop(emit);
            return Err(e);
        }
        drop(emit);
        Ok((
            if cancel.load(Ordering::Relaxed) {
                "cancelled".to_string()
            } else {
                "done".to_string()
            },
            json!({ "done": last_done, "failed": last_failed }),
        ))
    })
}

// ---------------------------------------------------------------- 只读报告 / 工具

fn handle_dupes_list(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match dupes::report(&conn) {
        Ok(groups) => success(id, groups),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OcrPendingParams {
    #[serde(default, alias = "root_id")]
    root_id: Option<i64>,
}

fn handle_ocr_pending(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: OcrPendingParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match ocr::count_pending(&conn, p.root_id) {
        Ok(n) => success(id, json!({ "pending": n })),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThumbEnsureParams {
    item_id: i64,
}

fn handle_thumb_ensure(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: ThumbEnsureParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let default_derived = headless_derived_dir();
    let overrides = state.tool_overrides();
    let ffmpeg = thumbs::ffmpeg_program(&overrides);
    match thumbs::ensure_thumb(&conn, p.item_id, &default_derived, &ffmpeg) {
        Ok(r) => {
            if r.created {
                let _ = state.audit_now(&conn, "create", "thumb", Some(p.item_id), json!({}));
            }
            success(id, r)
        }
        Err(e) => internal(id, e),
    }
}

fn handle_backup_now(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let overrides = state.tool_overrides();
    let default_derived = headless_derived_dir();
    let runner = backup::CliRestic::from_overrides(&overrides);
    match backup::run_backup(&mut conn, &default_derived, Some(&runner)) {
        Ok(report) => {
            let _ = state.audit_now(
                &conn,
                "create",
                "backup",
                None,
                json!({
                    "snapshot": report.snapshot_path,
                    "kept": report.kept,
                    "restic": report.restic.is_some(),
                }),
            );
            success(id, report)
        }
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortPathParams {
    path: String,
}

fn handle_port_export(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: PortPathParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match port::export_doc(&conn) {
        Ok(doc) => {
            let json_str = match serde_json::to_string_pretty(&doc) {
                Ok(j) => j,
                Err(e) => return internal(id, format!("序列化失败: {e}")),
            };
            if let Err(e) = std::fs::write(&p.path, json_str) {
                return internal(id, format!("写入文件失败: {e}"));
            }
            let n = doc.categories.len() + doc.tags.len();
            let _ = state.audit_now(
                &conn,
                "create",
                "export",
                None,
                json!({
                    "path": p.path,
                    "categories": doc.categories.len(),
                    "tags": doc.tags.len(),
                }),
            );
            success(id, json!({ "count": n }))
        }
        Err(e) => internal(id, e),
    }
}

fn handle_port_import(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: PortPathParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    let json_str = match std::fs::read_to_string(&p.path) {
        Ok(j) => j,
        Err(e) => return internal(id, format!("读取文件失败: {e}")),
    };
    match port::import_json(&mut conn, &json_str) {
        Ok(report) => {
            let _ = state.audit_now(
                &conn,
                "create",
                "import",
                None,
                serde_json::to_value(&report).unwrap_or(json!({})),
            );
            success(id, report)
        }
        Err(e) => invalid_params(id, e),
    }
}

// ---------------------------------------------------------------- 用户 / 登录 / 审计

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminSetupParams {
    username: String,
    password: String,
}

fn handle_admin_setup(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: AdminSetupParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::setup_admin(&conn, &p.username, &p.password) {
        Ok(uid) => {
            match auth::list_users(&conn)
                .map_err(|e| e.to_string())
                .and_then(|users| {
                    users
                        .into_iter()
                        .find(|u| u.id == uid)
                        .ok_or_else(|| "创建后读取用户失败".to_string())
                }) {
                Ok(user) => {
                    state.auth.login(user.clone());
                    let _ = state.audit_now(
                        &conn,
                        "setup",
                        "user",
                        Some(uid),
                        json!({ "username": user.username, "role": user.role }),
                    );
                    success(id, user)
                }
                Err(e) => internal(id, e),
            }
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthLoginParams {
    username: String,
    password: String,
}

fn handle_auth_login(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: AuthLoginParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::authenticate(&conn, &p.username, &p.password) {
        Ok(Some(user)) => {
            state.auth.login(user.clone());
            let _ = state.audit_now(
                &conn,
                "login",
                "session",
                Some(user.id),
                json!({ "username": user.username }),
            );
            success(id, user)
        }
        Ok(None) => {
            let _ = audit::record(
                &conn,
                None,
                "login",
                "session",
                None,
                &json!({ "ok": false, "username": p.username.trim() }).to_string(),
            );
            RpcResponse::error(
                Some(id),
                RpcError::new(codes::PERMISSION_DENIED, "用户名或密码错误"),
            )
        }
        Err(e) => internal(id, format!("登录失败: {e}")),
    }
}

fn handle_auth_logout(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    if let Some(user) = state.auth.current() {
        if let Ok(conn) = state.pool.get() {
            let _ = state.audit_now(&conn, "logout", "session", Some(user.id), json!({}));
        }
        state.auth.logout();
    }
    success(id, json!({}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordChangeParams {
    old_password: String,
    new_password: String,
}

fn handle_password_change(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let p: PasswordChangeParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let user = match state.auth.current() {
        Some(u) => u,
        None => {
            return RpcResponse::error(
                Some(id),
                RpcError::new(codes::PERMISSION_DENIED, "未登录"),
            )
        }
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::change_password(&conn, user.id, &p.old_password, &p.new_password) {
        Ok(()) => {
            let _ = state.audit_now(&conn, "password", "user", Some(user.id), json!({}));
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

fn handle_users_list(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::list_users(&conn) {
        Ok(users) => success(id, users),
        Err(e) => internal(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserCreateParams {
    username: String,
    password: String,
    role: String,
}

fn handle_user_create(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: UserCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::create_user(&conn, &p.username, &p.password, &p.role) {
        Ok(uid) => {
            let _ = state.audit_now(
                &conn,
                "create",
                "user",
                Some(uid),
                json!({ "username": p.username.trim(), "role": p.role }),
            );
            success(id, json!({ "id": uid }))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDeleteParams {
    user_id: i64,
}

fn handle_user_delete(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: UserDeleteParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    if state.auth.user_id() == Some(p.user_id) {
        return invalid_params(id, "不能删除当前登录用户");
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::delete_user(&conn, p.user_id) {
        Ok(()) => {
            let _ = state.audit_now(&conn, "delete", "user", Some(p.user_id), json!({}));
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserRoleSetParams {
    user_id: i64,
    role: String,
}

fn handle_user_role_set(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: UserRoleSetParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match auth::set_role(&conn, p.user_id, &p.role) {
        Ok(()) => {
            let _ = state.audit_now(
                &conn,
                "update",
                "user",
                Some(p.user_id),
                json!({ "role": p.role }),
            );
            success(id, json!({}))
        }
        Err(e) => invalid_params(id, e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditListParams {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    entity: Option<String>,
}

fn handle_audit_list(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: AuditListParams = match parse_params(params) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, e),
    };
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return internal(id, format!("数据库连接失败: {e}")),
    };
    match audit::recent(&conn, p.limit.unwrap_or(50), p.entity.as_deref()) {
        Ok(rows) => success(id, rows),
        Err(e) => internal(id, e),
    }
}
