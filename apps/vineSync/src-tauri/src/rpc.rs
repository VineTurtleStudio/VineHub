//! `--rpc` 模式：sidecar JSON-RPC over stdio（协议 v1，vineSync）。
//!
//! - argv 最早分流进入本模块，跳过 Tauri GUI / 单实例 / 托盘 / 触发器线程
//! - 方法面：`sync.preview`（试算不落盘）/ `sync.start`（执行同步）/ `task.cancel`
//! - ack 先回 taskId，进度走 `task.progress`，终态以同 id 第二响应返回
//! - 配置读取与 GUI 相同路径（便携模式或 %APPDATA%\studio.vineturtle.vinesync\config.json）

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

use crate::model::{ChangeKind, Config, Counts, DeletePolicy, PairConfig};
use crate::runtime;

const PLUGIN_ID: &str = "vine.sync";
const CAPABILITIES: &[&str] = &["preview", "sync", "cancel"];

struct TaskEntry {
    cancel: Arc<AtomicBool>,
}

struct RpcState {
    stdout: Mutex<std::io::Stdout>,
    tasks: Mutex<HashMap<String, TaskEntry>>,
    next_task: AtomicU64,
    /// GUI 与 RPC 共用同一 config.json；每次请求现读（不缓存）保证一致。
    app_dir: PathBuf,
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
            Err(e) => eprintln!("[vinesync] 编码消息失败: {}", e.message),
        }
    }

    fn send_notification(&self, method_name: &str, params: impl Serialize) {
        let mut note = RpcRequest::notification(method_name);
        match serde_json::to_value(params) {
            Ok(v) => note.params = Some(v),
            Err(e) => {
                eprintln!("[vinesync] 通知参数序列化失败: {e}");
                return;
            }
        }
        self.send_message(&note);
    }

    fn load_config(&self) -> Config {
        crate::storage::config::load(&self.app_dir.join("config.json"))
    }
}

fn headless_app_dir() -> PathBuf {
    if let Some(portable) = crate::storage::paths::portable_data_dir() {
        return portable;
    }
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    appdata.join("studio.vineturtle.vinesync")
}

/// RPC 模式主循环（stdin EOF 即退出）。
pub fn run() -> ExitCode {
    eprintln!(
        "[vinesync] rpc mode started (api {}, pid {})",
        vine_plugin_api::HOST_API_VERSION,
        std::process::id()
    );

    let state = Arc::new(RpcState {
        stdout: Mutex::new(std::io::stdout()),
        tasks: Mutex::new(HashMap::new()),
        next_task: AtomicU64::new(0),
        app_dir: headless_app_dir(),
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
                eprintln!("[vinesync] 无法解析的请求行: {}", e.message);
                state.send_message(&RpcResponse::error(None, e.into_rpc_error()));
            }
        }
    }

    eprintln!("[vinesync] rpc stdin closed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let active = state.tasks.lock().expect("tasks lock").len();
        if active == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("[vinesync] 等待在途任务超时（{active} 个），强制退出");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    eprintln!("[vinesync] rpc mode exit");
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
                        eprintln!("[vinesync] task {} cancel requested (notification)", params.task_id);
                    }
                }
                Err(e) => eprintln!("[vinesync] task.cancel 参数非法: {e}"),
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
        method::SYNC_PREVIEW => handle_sync_preview(state, id, req.params),
        method::SYNC_START => handle_sync_start(state, id, req.params),
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

/// 同步请求参数：`pairId` 引用 config.json 中的对；或直接给 source/dest 临时对。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncParams {
    #[serde(default)]
    pair_id: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    dest: Option<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    delete_policy: Option<DeletePolicy>,
}

fn resolve_pair(state: &RpcState, params: &SyncParams) -> Result<(PairConfig, Config), String> {
    let mut config = state.load_config();
    let pair = if let Some(id) = &params.pair_id {
        config
            .pairs
            .iter()
            .find(|p| &p.id == id)
            .cloned()
            .ok_or_else(|| format!("配置中不存在同步对: {id}"))?
    } else {
        let source = params
            .source
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .ok_or("缺少 source（或 pairId）")?;
        let dest = params
            .dest
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .ok_or("缺少 dest（或 pairId）")?;
        PairConfig {
            id: "adhoc".into(),
            name: format!("adhoc {}", std::process::id()),
            source: source.clone(),
            dest: dest.clone(),
            enabled: true,
            include: params.include.clone(),
            exclude: params.exclude.clone(),
            interval_minutes: None,
        }
    };
    if crate::model::dest_root_unsafe(&pair) {
        return Err("目标不能是驱动器根目录".into());
    }
    if !std::path::Path::new(&pair.source).is_dir() {
        return Err(format!("源目录不存在: {}", pair.source));
    }
    if let Some(policy) = params.delete_policy {
        config.delete_policy = policy;
    }
    Ok((pair, config))
}

fn spawn_task(state: &Arc<RpcState>) -> Result<(String, Arc<AtomicBool>), RpcError> {
    let task_id = format!("t-{}", state.next_task.fetch_add(1, Ordering::SeqCst) + 1);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tasks = state.tasks.lock().expect("tasks lock");
    if tasks.len() >= 2 {
        return Err(RpcError::new(
            codes::PLUGIN_INTERNAL,
            "并发任务已达上限（2），请稍后再试",
        ));
    }
    tasks.insert(task_id.clone(), TaskEntry { cancel: cancel.clone() });
    Ok((task_id, cancel))
}

fn counts_json(c: &Counts) -> Value {
    json!({
        "added": c.added,
        "updated": c.updated,
        "deleted": c.deleted,
        "versioned": c.versioned,
        "skipped": c.skipped,
        "failed": c.failed,
    })
}

fn handle_sync_preview(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: SyncParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let (pair, config) = match resolve_pair(state, &params) {
        Ok(v) => v,
        Err(e) => return invalid_params(id, e),
    };
    let (task_id, cancel) = match spawn_task(state) {
        Ok(t) => t,
        Err(e) => return RpcResponse::error(Some(id), e),
    };

    let ack = success(id.clone(), ScanStartResult { task_id: task_id.clone() });
    let state = state.clone();
    let terminal_id = id;
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let task = task_id.clone();
        state.send_notification(
            method::TASK_PROGRESS,
            TaskProgressParams {
                task_id: task.clone(),
                done: 0,
                total: 2,
                msg: Some("扫描源与目标…".into()),
            },
        );

        let terminal = match runtime::compute_changes(None, &pair, &config) {
            Ok((_src, _dst, d, skipped)) => {
                let mut counts = Counts::default();
                let mut total_bytes = 0u64;
                let mut changes = Vec::new();
                for ch in &d.changes {
                    match ch.kind {
                        ChangeKind::Add => counts.added += 1,
                        ChangeKind::Update => counts.updated += 1,
                        ChangeKind::Delete => counts.deleted += 1,
                        ChangeKind::Versioned => counts.versioned += 1,
                    }
                    total_bytes += ch.size;
                    if changes.len() < 1000 {
                        changes.push(json!({
                            "pairId": pair.id,
                            "pairName": pair.name,
                            "kind": ch.kind,
                            "relPath": ch.rel,
                            "size": ch.size,
                        }));
                    }
                }
                json!({
                    "taskId": task,
                    "pairId": pair.id,
                    "counts": counts_json(&counts),
                    "totalBytes": total_bytes,
                    "suppressed": d.suppressed,
                    "skipped": skipped,
                    "truncated": d.changes.len() > 1000,
                    "changeCount": d.changes.len(),
                    "changes": changes,
                    "cancelled": cancel.load(Ordering::SeqCst),
                    "elapsedMs": start.elapsed().as_millis() as u64,
                })
            }
            Err(e) => json!({
                "taskId": task,
                "error": e,
                "cancelled": cancel.load(Ordering::SeqCst),
            }),
        };
        state.send_message(&success(terminal_id, terminal));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vinesync] preview task {task_id} finished");
    });

    ack
}

fn handle_sync_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: SyncParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let (pair, config) = match resolve_pair(state, &params) {
        Ok(v) => v,
        Err(e) => return invalid_params(id, e),
    };
    let (task_id, cancel) = match spawn_task(state) {
        Ok(t) => t,
        Err(e) => return RpcResponse::error(Some(id), e),
    };

    let ack = success(id.clone(), ScanStartResult { task_id: task_id.clone() });
    let state = state.clone();
    let terminal_id = id;
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let task = task_id.clone();
        let progress_task = task_id.clone();
        state.send_notification(
            method::TASK_PROGRESS,
            TaskProgressParams {
                task_id: progress_task.clone(),
                done: 0,
                total: 100,
                msg: Some("扫描并计算差异…".into()),
            },
        );

        let terminal = (|| {
            let (_src, _dst, d, _skipped) = runtime::compute_changes(None, &pair, &config)?;
            let total = d.changes.len();
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: progress_task.clone(),
                    done: 0,
                    total: total as u64,
                    msg: Some(format!("开始应用 {total} 项变更")),
                },
            );
            let threads = config.copy_threads.clamp(1, 16) as usize;
            let paused = AtomicBool::new(false);
            let (counts, errors) = runtime::apply_changes(
                &pair,
                &config,
                &d.changes,
                threads,
                &paused,
                &cancel,
                |done, ch| {
                    if done % 50 == 0 || done == total {
                        state.send_notification(
                            method::TASK_PROGRESS,
                            TaskProgressParams {
                                task_id: progress_task.clone(),
                                done: done as u64,
                                total: total as u64,
                                msg: Some(format!("{:?} {}", ch.kind, ch.rel)),
                            },
                        );
                    }
                },
            );
            let cancelled = cancel.load(Ordering::SeqCst);
            Ok::<_, String>(json!({
                "taskId": task,
                "pairId": pair.id,
                "ok": counts.failed == 0,
                "counts": counts_json(&counts),
                "totalChanges": total,
                "failures": errors.iter().take(20).cloned().collect::<Vec<_>>(),
                "failureCount": errors.len(),
                "cancelled": cancelled,
                "elapsedMs": start.elapsed().as_millis() as u64,
            }))
        })();

        let payload = match terminal {
            Ok(v) => v,
            Err(e) => json!({
                "taskId": task,
                "ok": false,
                "error": e,
                "cancelled": cancel.load(Ordering::SeqCst),
            }),
        };
        state.send_message(&success(terminal_id, payload));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vinesync] sync task {task_id} finished");
    });

    ack
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
            eprintln!("[vinesync] task {} cancel requested", params.task_id);
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
