//! `--rpc` 模式：sidecar JSON-RPC over stdio（协议 v1，vineCurator）。
//!
//! - argv 最早分流，跳过 Tauri GUI / asset 协议作用域
//! - 方法面：`library.addRoot` / `library.roots` / `library.scan` /
//!   `search.items`（FTS5 优先、LIKE 兜底）/ `task.cancel`
//! - 打开与 GUI 同一 catalog.sqlite（WAL + busy_timeout，多进程可并存）；
//!   hub 侧以只读查询为主，scan/addRoot 为受控写入

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

use crate::{browse, db, fts, scan, smart};

const PLUGIN_ID: &str = "vine.curator";
const CAPABILITIES: &[&str] = &["library", "search", "cancel"];

struct TaskEntry {
    cancel: Arc<AtomicBool>,
}

struct RpcState {
    stdout: Mutex<std::io::Stdout>,
    tasks: Mutex<HashMap<String, TaskEntry>>,
    next_task: AtomicU64,
    pool: db::Pool,
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

/// 与 GUI setup 等价的最小引导：开池 → 迁移 → FTS 一致性 → 种子集合。
fn bootstrap() -> Result<db::Pool, String> {
    let path = headless_db_path();
    let pool = db::open_pool(&path).map_err(|e| format!("打开数据库失败: {e}"))?;
    {
        let mut conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
        db::migrate::migrate(&mut conn);
        fts::ensure_consistent(&mut conn).map_err(|e| format!("FTS 一致性检查失败: {e}"))?;
        smart::seed_example_collections(&conn).map_err(|e| format!("种子集合失败: {e}"))?;
    }
    Ok(pool)
}

/// RPC 模式主循环（stdin EOF 即退出）。
pub fn run() -> ExitCode {
    eprintln!(
        "[vine-curator] rpc mode started (api {}, pid {})",
        vine_plugin_api::HOST_API_VERSION,
        std::process::id()
    );

    let pool = match bootstrap() {
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
        method::LIBRARY_ADD_ROOT => handle_add_root(state, id, req.params),
        method::LIBRARY_ROOTS => handle_roots(state, id),
        method::LIBRARY_SCAN => handle_library_scan(state, id, req.params),
        method::SEARCH_ITEMS => handle_search(state, id, req.params),
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

fn root_json(r: &db::repo::RootEntry) -> Value {
    json!({
        "id": r.id,
        "path": r.path,
        "label": r.label,
        "itemCount": r.item_count,
        "createdAt": r.created_at,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddRootParams {
    path: String,
    #[serde(default)]
    label: Option<String>,
}

fn handle_add_root(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: AddRootParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e.to_string())),
    };
    let label = params
        .label
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(&params.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| params.path.clone())
        });
    match db::repo::add_root(&mut conn, &params.path, &label) {
        Ok(entry) => success(id, root_json(&entry)),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::INVALID_PARAMS, e)),
    }
}

fn handle_roots(state: &Arc<RpcState>, id: RequestId) -> RpcResponse {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e.to_string())),
    };
    match db::repo::list_roots(&conn) {
        Ok(roots) => success(
            id,
            json!({ "roots": roots.iter().map(root_json).collect::<Vec<_>>() }),
        ),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e.to_string())),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryScanParams {
    root_id: i64,
}

fn handle_library_scan(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: LibraryScanParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };

    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e.to_string())),
    };
    let root_path = match db::repo::get_root_path(&conn, params.root_id) {
        Ok(p) => PathBuf::from(p),
        Err(e) => return invalid_params(id, e),
    };
    drop(conn);

    let task_id = format!("t-{}", state.next_task.fetch_add(1, Ordering::SeqCst) + 1);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut tasks = state.tasks.lock().expect("tasks lock");
        if tasks.len() >= 2 {
            return RpcResponse::error(
                Some(id),
                RpcError::new(codes::PLUGIN_INTERNAL, "并发任务已达上限（2）"),
            );
        }
        tasks.insert(task_id.clone(), TaskEntry { cancel: cancel.clone() });
    }

    let ack = success(id.clone(), ScanStartResult { task_id: task_id.clone() });
    let state = state.clone();
    let terminal_id = id;
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let task = task_id.clone();
        let progress_task = task_id.clone();
        let outcome = (|| {
            let mut conn = state
                .pool
                .get()
                .map_err(|e| format!("获取数据库连接失败: {e}"))?;
            scan::scan_root(&mut conn, params.root_id, &root_path, &cancel, &mut |p| {
                state.send_notification(
                    method::TASK_PROGRESS,
                    TaskProgressParams {
                        task_id: progress_task.clone(),
                        done: p.index as u64,
                        total: p.total as u64,
                        msg: Some(format!("{} {}", p.phase, p.rel_path)),
                    },
                );
            })
        })();
        let payload = match outcome {
            Ok(o) => json!({
                "taskId": task,
                "status": o.status,
                "stats": {
                    "seen": o.stats.seen,
                    "changed": o.stats.changed,
                    "unchanged": o.stats.unchanged,
                    "missing": o.stats.missing,
                    "errors": o.stats.errors,
                },
                "cancelled": cancel.load(Ordering::SeqCst),
                "elapsedMs": start.elapsed().as_millis() as u64,
            }),
            Err(e) => json!({
                "taskId": task,
                "status": "error",
                "error": e,
                "cancelled": cancel.load(Ordering::SeqCst),
            }),
        };
        state.send_message(&success(terminal_id, payload));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vine-curator] scan task {task_id} finished");
    });

    ack
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    query: String,
    #[serde(default)]
    limit: Option<i64>,
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

fn handle_search(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: SearchParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    if params.query.trim().is_empty() {
        return invalid_params(id, "query 不能为空");
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e.to_string())),
    };
    match browse::search_items(&conn, &params.query, params.limit) {
        Ok(items) => success(
            id,
            json!({
                "total": items.len(),
                "items": items.iter().map(item_json).collect::<Vec<_>>(),
            }),
        ),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}

fn handle_task_cancel(state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: TaskCancelParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };

    let tasks = state.tasks.lock().expect("tasks lock");
    match tasks.get(&params.task_id) {
        Some(task) => {
            task.cancel.store(true, Ordering::SeqCst);
            drop(tasks);
            eprintln!("[vine-curator] task {} cancel requested", params.task_id);
            success(
                id,
                json!({ "ok": true, "taskId": params.task_id, "msg": "已请求取消，终态稍后送达" }),
            )
        }
        None => RpcResponse::error(
            Some(id),
            RpcError::new(codes::TASK_NOT_FOUND, format!("任务不存在: {}", params.task_id)),
        ),
    }
}
