//! `--rpc` 模式：sidecar JSON-RPC over stdio（协议 v1）。
//!
//! - argv 最早分流进入本模块，跳过控制台 UTF-8 初始化与 CLI 解析
//! - stdin/stdout 一行一条 UTF-8 JSON；stderr 一律日志
//! - `scan.start`：ack 先回 taskId，扫描在线程中跑，进度走 `task.progress`
//!   通知，终态以同 id 第二响应返回（id 配对 = 终态）
//! - `task.cancel`：`AtomicBool` 协作中断，扫描线程尽快回终态

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};

use vine_plugin_api::{
    PluginInfo, RequestId, RpcError, RpcRequest, RpcResponse, ScanStartParams, ScanStartResult,
    TaskCancelParams, TaskProgressParams, codes, method, parse_request_line, to_line,
};

use crate::{walker, Opts};

const PLUGIN_ID: &str = "vine.pulse";
const CAPABILITIES: &[&str] = &["scan", "report", "cancel"];

struct TaskEntry {
    cancel: Arc<AtomicBool>,
}

struct RpcState {
    stdout: Mutex<std::io::Stdout>,
    tasks: Mutex<HashMap<String, TaskEntry>>,
    next_task: AtomicU64,
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
            Err(e) => eprintln!("[vine-pulse] 编码消息失败: {}", e.message),
        }
    }

    fn send_notification(&self, method_name: &str, params: impl Serialize) {
        let mut note = RpcRequest::notification(method_name);
        match serde_json::to_value(params) {
            Ok(v) => note.params = Some(v),
            Err(e) => {
                eprintln!("[vine-pulse] 通知参数序列化失败: {e}");
                return;
            }
        }
        self.send_message(&note);
    }
}

/// RPC 模式主循环（stdin EOF 即退出）。
pub fn run() -> ExitCode {
    eprintln!(
        "[vine-pulse] rpc mode started (api {}, pid {})",
        vine_plugin_api::HOST_API_VERSION,
        std::process::id()
    );

    let state = Arc::new(RpcState {
        stdout: Mutex::new(std::io::stdout()),
        tasks: Mutex::new(HashMap::new()),
        next_task: AtomicU64::new(0),
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
                eprintln!("[vine-pulse] 无法解析的请求行: {}", e.message);
                state.send_message(&RpcResponse::error(None, e.into_rpc_error()));
            }
        }
    }

    eprintln!("[vine-pulse] rpc stdin closed");

    // 等待在途任务回终态（最多 10s），避免宿主关闭输入时丢失终态
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let active = state.tasks.lock().expect("tasks lock").len();
        if active == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("[vine-pulse] 等待在途任务超时（{active} 个），强制退出");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    eprintln!("[vine-pulse] rpc mode exit");
    ExitCode::SUCCESS
}

fn handle_request(state: &Arc<RpcState>, req: RpcRequest) {
    // 通知（无 id）：task.cancel 按规范以通知形式发送
    let Some(id) = req.id.clone() else {
        if req.method == method::TASK_CANCEL {
            match serde_json::from_value::<TaskCancelParams>(req.params.clone().unwrap_or(Value::Null))
            {
                Ok(params) => {
                    let tasks = state.tasks.lock().expect("tasks lock");
                    if let Some(task) = tasks.get(&params.task_id) {
                        task.cancel.store(true, Ordering::SeqCst);
                        eprintln!("[vine-pulse] task {} cancel requested (notification)", params.task_id);
                    }
                }
                Err(e) => eprintln!("[vine-pulse] task.cancel 参数非法: {e}"),
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
        method::SCAN_START => handle_scan_start(state, id, req.params),
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

fn handle_scan_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: ScanStartParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let deep = params.deep.unwrap_or(false);
    let full = params.full.unwrap_or(false);

    let path = PathBuf::from(&params.path);
    if !path.exists() {
        return invalid_params(id, format!("路径不存在: {}", params.path));
    }

    // 类型过滤：扩展名小写、不含点
    let types: Vec<String> = params
        .types
        .iter()
        .map(|t| t.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    let (files, errors) = walker::collect_files(&[path]);
    for e in &errors {
        eprintln!("[vine-pulse] 收集文件警告: {e}");
    }
    let files: Vec<(PathBuf, u64)> = if types.is_empty() {
        files
    } else {
        files
            .into_iter()
            .filter(|(p, _)| {
                p.extension()
                    .map(|e| types.contains(&e.to_string_lossy().to_ascii_lowercase()))
                    .unwrap_or(false)
            })
            .collect()
    };

    if files.is_empty() {
        return invalid_params(id, "没有找到可扫描的文件（检查路径或类型过滤）");
    }

    let task_id = format!("t-{}", state.next_task.fetch_add(1, Ordering::SeqCst) + 1);
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .tasks
        .lock()
        .expect("tasks lock")
        .insert(task_id.clone(), TaskEntry { cancel: cancel.clone() });

    // ack（第一响应）：立即回 taskId
    let ack = success(id.clone(), ScanStartResult { task_id: task_id.clone() });

    // 扫描线程：进度通知 + 终态（同 id 第二响应）
    let state = state.clone();
    let terminal_id = id;
    std::thread::spawn(move || {
        let opts = Opts {
            deep,
            full,
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let total = files.len();
        let task_for_progress = task_id.clone();
        let reports = walker::scan(&files, &opts, &cancel, |done, t, _report| {
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: task_for_progress.clone(),
                    done: done as u64,
                    total: t as u64,
                    msg: None,
                },
            );
        });

        let cancelled = cancel.load(Ordering::SeqCst);
        let terminal = terminal_result(&task_id, &reports, cancelled, start.elapsed(), total);
        state.send_message(&success(terminal_id, terminal));
        state
            .tasks
            .lock()
            .expect("tasks lock")
            .remove(&task_id);
        eprintln!(
            "[vine-pulse] task {task_id} finished (cancelled={cancelled}, {} files)",
            reports.len()
        );
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
            eprintln!("[vine-pulse] task {} cancel requested", params.task_id);
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

/// 终态载荷：汇总计数 + 完整报告（供 hub 结果表渲染）。
fn terminal_result(
    task_id: &str,
    reports: &[crate::Report],
    cancelled: bool,
    elapsed: std::time::Duration,
    discovered: usize,
) -> Value {
    let mut ok = 0u64;
    let mut bad = 0u64;
    let mut warning = 0u64;
    let mut corrupt = 0u64;
    let mut unknown = 0u64;
    for r in reports {
        if r.is_ok() {
            ok += 1;
        } else {
            bad += 1;
        }
        match r.worst() {
            crate::Severity::Warning => warning += 1,
            crate::Severity::Corrupt => corrupt += 1,
            crate::Severity::Unknown => unknown += 1,
            crate::Severity::Ok => {}
        }
    }
    json!({
        "taskId": task_id,
        "ok": ok,
        "bad": bad,
        "warning": warning,
        "corrupt": corrupt,
        "unknown": unknown,
        "scanned": reports.len(),
        "discovered": discovered,
        "cancelled": cancelled,
        "elapsedMs": elapsed.as_millis() as u64,
        "reports": reports,
    })
}
