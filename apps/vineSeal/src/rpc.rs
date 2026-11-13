use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};

use vine_plugin_api::{
    codes, method, parse_request_line, to_line, PluginInfo, RequestId, RpcError, RpcRequest,
    RpcResponse, TaskCancelParams, TaskProgressParams, TransferStartParams, TransferStartResult,
    VerifyStartParams, VerifyStartResult,
};

use crate::core::conflict::ConflictPolicy;
use crate::core::hash::HashAlgo;
use crate::core::paths::available_space;
use crate::core::plan::{build_plan, concat};
use crate::core::transfer::{run_transfer, TransferMode, TransferOptions};
use crate::core::verify::{parse_manifest, verify_against_manifest, VerifyItem};

const PLUGIN_ID: &str = "vine.seal";
const CAPABILITIES: &[&str] = &["transfer", "verify", "cancel"];

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
            Err(e) => eprintln!("[vine-seal] 编码消息失败: {}", e.message),
        }
    }

    fn send_notification(&self, method_name: &str, params: impl Serialize) {
        let mut note = RpcRequest::notification(method_name);
        match serde_json::to_value(params) {
            Ok(v) => note.params = Some(v),
            Err(e) => {
                eprintln!("[vine-seal] 通知参数序列化失败: {e}");
                return;
            }
        }
        self.send_message(&note);
    }
}

pub fn run() -> ExitCode {
    eprintln!(
        "[vine-seal] rpc mode started (api {}, pid {})",
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
                eprintln!("[vine-seal] 无法解析的请求行: {}", e.message);
                state.send_message(&RpcResponse::error(None, e.into_rpc_error()));
            }
        }
    }

    eprintln!("[vine-seal] rpc stdin closed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let active = state.tasks.lock().expect("tasks lock").len();
        if active == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("[vine-seal] 等待在途任务超时（{active} 个），强制退出");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    eprintln!("[vine-seal] rpc mode exit");
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
                            "[vine-seal] task {} cancel requested (notification)",
                            params.task_id
                        );
                    }
                }
                Err(e) => eprintln!("[vine-seal] task.cancel 参数非法: {e}"),
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
        method::TRANSFER_START => handle_transfer_start(state, id, req.params),
        method::VERIFY_START => handle_verify_start(state, id, req.params),
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

fn next_task_id(state: &Arc<RpcState>) -> String {
    format!("t-{}", state.next_task.fetch_add(1, Ordering::SeqCst) + 1)
}

fn handle_transfer_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: TransferStartParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    if params.sources.is_empty() {
        return invalid_params(id, "sources 不能为空");
    }
    let mode = match params.mode.as_deref() {
        None | Some("copy") => TransferMode::Copy,
        Some("move") => TransferMode::Move,
        Some(other) => return invalid_params(id, format!("mode 非法: {other}（应为 copy/move）")),
    };
    let dest = PathBuf::from(&params.dest);
    if !dest.is_dir() {
        return invalid_params(id, format!("目标目录不存在: {}", params.dest));
    }

    let mut plans = Vec::new();
    for src in &params.sources {
        let src_path = PathBuf::from(src);
        if !src_path.exists() {
            return invalid_params(id, format!("源不存在: {src}"));
        }
        match build_plan(&src_path, &dest, ConflictPolicy::Rename) {
            Ok(p) => plans.push(p),
            Err(e) => return invalid_params(id, e.to_string()),
        }
    }
    let plan = concat(plans);
    if plan.items.is_empty() {
        return invalid_params(id, "计划为空（没有可转移的文件）");
    }
    if let Ok(free) = available_space(&dest)
        && plan.transfer_bytes > free
    {
        return invalid_params(
            id,
            format!(
                "目标空间不足：需要 {} 字节，可用 {free} 字节",
                plan.transfer_bytes
            ),
        );
    }

    let task_id = next_task_id(state);
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .tasks
        .lock()
        .expect("tasks lock")
        .insert(task_id.clone(), TaskEntry { cancel: cancel.clone() });

    let ack = success(
        id.clone(),
        TransferStartResult {
            task_id: task_id.clone(),
        },
    );

    let state_t = state.clone();
    let terminal_id = id;
    let files_total = plan.items.len() as u32;
    std::thread::spawn(move || {
        let opts = TransferOptions {
            mode,
            ..Default::default()
        };
        let task_for_progress = task_id.clone();
        let summary = run_transfer(
            &plan,
            &opts,
            |p| {
                state_t.send_notification(
                    method::TASK_PROGRESS,
                    TaskProgressParams {
                        task_id: task_for_progress.clone(),
                        done: p.files_done as u64,
                        total: files_total as u64,
                        msg: Some(p.current.clone()),
                    },
                );
            },
            &cancel,
        );
        let cancelled = cancel.load(Ordering::SeqCst);
        let terminal = json!({
            "taskId": task_id,
            "total": summary.total,
            "ok": summary.ok,
            "bad": summary.bad,
            "skipped": summary.skipped,
            "cancelled": cancelled || summary.cancelled > 0,
            "bytes": summary.bytes,
            "outcomes": summary.outcomes,
        });
        state_t.send_message(&success(terminal_id, terminal));
        state_t
            .tasks
            .lock()
            .expect("tasks lock")
            .remove(&task_id);
        eprintln!(
            "[vine-seal] task {task_id} finished (ok={}, bad={})",
            summary.ok, summary.bad
        );
    });

    ack
}

fn handle_verify_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: VerifyStartParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let manifest_path = PathBuf::from(&params.manifest);
    if !manifest_path.is_file() {
        return invalid_params(id, format!("清单不存在: {}", params.manifest));
    }
    let base = PathBuf::from(&params.base);
    if !base.is_dir() {
        return invalid_params(id, format!("基目录不存在: {}", params.base));
    }
    let algo = match params.algo.as_deref() {
        None | Some("sha256") => HashAlgo::Sha256,
        Some("xxh3") => HashAlgo::Xxh3,
        Some("crc32") => HashAlgo::Crc32,
        Some("md5") => HashAlgo::Md5,
        Some(other) => return invalid_params(id, format!("algo 非法: {other}")),
    };

    let text = match std::fs::read_to_string(crate::core::paths::to_long_path(&manifest_path)) {
        Ok(t) => t,
        Err(e) => return invalid_params(id, format!("读取清单失败：{e}")),
    };
    let entries = parse_manifest(&text);
    if entries.is_empty() {
        return invalid_params(id, "清单为空或无法解析");
    }

    let task_id = next_task_id(state);
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .tasks
        .lock()
        .expect("tasks lock")
        .insert(task_id.clone(), TaskEntry { cancel: cancel.clone() });

    let ack = success(
        id.clone(),
        VerifyStartResult {
            task_id: task_id.clone(),
        },
    );

    let state_t = state.clone();
    let terminal_id = id;
    std::thread::spawn(move || {
        let items: Vec<VerifyItem> =
            verify_against_manifest(&entries, &base, algo, |done, t| {
                state_t.send_notification(
                    method::TASK_PROGRESS,
                    TaskProgressParams {
                        task_id: task_id.clone(),
                        done: done as u64,
                        total: t as u64,
                        msg: None,
                    },
                );
            });
        let (ok, missing, mismatch, read_error) = crate::core::verify::summarize(&items);
        let terminal = json!({
            "taskId": task_id,
            "total": items.len(),
            "ok": ok,
            "missing": missing,
            "mismatch": mismatch,
            "readError": read_error,
            "items": items,
        });
        state_t.send_message(&success(terminal_id, terminal));
        state_t
            .tasks
            .lock()
            .expect("tasks lock")
            .remove(&task_id);
        eprintln!(
            "[vine-seal] verify {task_id} finished (ok={ok}, mismatch={mismatch})"
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
            eprintln!("[vine-seal] task {} cancel requested", params.task_id);
            success(
                id,
                json!({ "ok": true, "taskId": params.task_id, "msg": "已请求取消，终态稍后送达" }),
            )
        }
        None => RpcResponse::error(
            Some(id),
            RpcError::new(
                codes::TASK_NOT_FOUND,
                format!("任务不存在: {}", params.task_id),
            ),
        ),
    }
}
