//! `--rpc` 模式：sidecar JSON-RPC over stdio（协议 v1，VinePalmaCursor）。
//!
//! - argv 最早分流，跳过 Tauri GUI
//! - 方法面：`file.convert`（图片/cur/ani → cur 或 ani，落盘）/ `task.cancel` /
//!   P3.3 单响应路径读写：`file.exportCur`、`file.exportAni`、`file.importPath`、
//!   `import.file`（内存字节导入，bridge 专用）
//! - 无状态字节转换：无配置、无数据库；ack 先回 taskId，进度走 `task.progress`，
//!   终态以同 id 第二响应返回；路径导入导出为纯同步单响应（codec 无 IO 依赖）

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

use crate::codec::bitmap::{BitmapImage, CursorFrame};
use crate::codec::scale::InterpolationMode;
use crate::codec::{ani, cur, scale};

const PLUGIN_ID: &str = "vine.cursor";
const CAPABILITIES: &[&str] = &["convert", "cancel", "export", "import"];
/// 内存字节导入（宿主桥接专用；plugin-api 未定义该常量，方法名在本插件内固定）。
const METHOD_IMPORT_FILE: &str = "import.file";

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
            Err(e) => eprintln!("[vine-cursor] 编码消息失败: {}", e.message),
        }
    }

    fn send_notification(&self, method_name: &str, params: impl Serialize) {
        let mut note = RpcRequest::notification(method_name);
        match serde_json::to_value(params) {
            Ok(v) => note.params = Some(v),
            Err(e) => {
                eprintln!("[vine-cursor] 通知参数序列化失败: {e}");
                return;
            }
        }
        self.send_message(&note);
    }
}

/// RPC 模式主循环（stdin EOF 即退出）。
pub fn run() -> ExitCode {
    eprintln!(
        "[vine-cursor] rpc mode started (api {}, pid {})",
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
                eprintln!("[vine-cursor] 无法解析的请求行: {}", e.message);
                state.send_message(&RpcResponse::error(None, e.into_rpc_error()));
            }
        }
    }

    eprintln!("[vine-cursor] rpc stdin closed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let active = state.tasks.lock().expect("tasks lock").len();
        if active == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("[vine-cursor] 等待在途任务超时（{active} 个），强制退出");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    eprintln!("[vine-cursor] rpc mode exit");
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
                            "[vine-cursor] task {} cancel requested (notification)",
                            params.task_id
                        );
                    }
                }
                Err(e) => eprintln!("[vine-cursor] task.cancel 参数非法: {e}"),
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
        method::FILE_CONVERT => handle_file_convert(state, id, req.params),
        method::EXPORT_CUR_TO_PATH => handle_export_cur_to_path(id, req.params),
        method::EXPORT_ANI_TO_PATH => handle_export_ani_to_path(id, req.params),
        method::IMPORT_FROM_PATH => handle_import_from_path(id, req.params),
        METHOD_IMPORT_FILE => handle_import_file_bytes(id, req.params),
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileConvertParams {
    input_path: String,
    output_path: String,
    /// cur | ani
    target: String,
    /// 仅 cur：多尺寸输出（缺省 = 源尺寸单帧）
    #[serde(default)]
    sizes: Option<Vec<u32>>,
}

fn handle_file_convert(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: FileConvertParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let input = PathBuf::from(&params.input_path);
    if !input.is_file() {
        return invalid_params(id, format!("输入文件不存在: {}", params.input_path));
    }
    let target = params.target.to_ascii_lowercase();
    if target != "cur" && target != "ani" {
        return invalid_params(id, format!("target 仅支持 cur/ani，得到: {}", params.target));
    }
    if let Some(sizes) = &params.sizes {
        if sizes.is_empty() || sizes.iter().any(|&s| s == 0 || s > 256) {
            return invalid_params(id, "sizes 非法（1..=256）");
        }
    }

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
        let send_progress = |done: u64, msg: &str| {
            state.send_notification(
                method::TASK_PROGRESS,
                TaskProgressParams {
                    task_id: progress_task.clone(),
                    done,
                    total: 3,
                    msg: Some(msg.to_string()),
                },
            );
        };

        let outcome = (|| -> Result<Value, String> {
            send_progress(0, "读取并解码…");
            let data = std::fs::read(&input).map_err(|e| format!("读取输入失败: {e}"))?;
            let imported = crate::commands::import_file(data)
                .map_err(|e| format!("解码失败: {e}"))?;
            let frame_count = imported.frames.len();
            if frame_count == 0 {
                return Err("输入不含任何帧".into());
            }
            if cancel.load(Ordering::SeqCst) {
                return Err("已取消".into());
            }

            let first = &imported.frames[0];
            if first.rgba.len() != (first.width * first.height * 4) as usize {
                return Err(format!(
                    "首帧 RGBA 长度不匹配: {}x{} 需 {} 字节，得到 {}",
                    first.width,
                    first.height,
                    (first.width * first.height * 4),
                    first.rgba.len()
                ));
            }
            let source = BitmapImage::from_rgba(
                first.width,
                first.height,
                first.hotspot_x,
                first.hotspot_y,
                first.rgba.clone(),
            );

            send_progress(1, "编码…");
            let (out_bytes, sizes_json) = match target.as_str() {
                "cur" => {
                    let sizes: Vec<u32> = params
                        .sizes
                        .clone()
                        .unwrap_or_else(|| vec![first.width.max(first.height)]);
                    let mut images: Vec<BitmapImage> = Vec::with_capacity(sizes.len());
                    for &s in &sizes {
                        if cancel.load(Ordering::SeqCst) {
                            return Err("已取消".into());
                        }
                        images.push(scale::downsample(&source, s, s, InterpolationMode::Bilinear));
                    }
                    let bytes = cur::write_cur(&images)?;
                    (bytes, json!(sizes))
                }
                "ani" => {
                    let frames: Vec<CursorFrame> = imported
                        .frames
                        .iter()
                        .map(|f| {
                            if f.rgba.len() != (f.width * f.height * 4) as usize {
                                return Err(format!(
                                    "帧 RGBA 长度不匹配: {}x{}",
                                    f.width, f.height
                                ));
                            }
                            Ok(CursorFrame {
                                image: BitmapImage::from_rgba(
                                    f.width,
                                    f.height,
                                    f.hotspot_x,
                                    f.hotspot_y,
                                    f.rgba.clone(),
                                ),
                                delay_ms: f.delay_ms,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let bytes = ani::write_ani(&frames)?;
                    (bytes, Value::Null)
                }
                _ => unreachable!(),
            };
            if cancel.load(Ordering::SeqCst) {
                return Err("已取消".into());
            }

            send_progress(2, "写入…");
            if let Some(parent) = PathBuf::from(&params.output_path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("创建输出目录失败: {e}"))?;
                }
            }
            std::fs::write(&params.output_path, &out_bytes)
                .map_err(|e| format!("写入输出失败: {e}"))?;

            Ok(json!({
                "taskId": task,
                "target": target,
                "sourceFormat": imported.format,
                "frameCount": frame_count,
                "bytes": out_bytes.len(),
                "sizes": sizes_json,
                "outputPath": params.output_path,
                "cancelled": false,
                "elapsedMs": start.elapsed().as_millis() as u64,
            }))
        })();

        let payload = match outcome {
            Ok(v) => v,
            Err(e) => {
                let cancelled = e == "已取消";
                json!({
                    "taskId": task,
                    "error": e,
                    "cancelled": cancelled,
                    "elapsedMs": start.elapsed().as_millis() as u64,
                })
            }
        };
        state.send_message(&success(terminal_id, payload));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vine-cursor] convert task {task_id} finished");
    });

    ack
}

// ---------- P3.3 路径导入导出（单响应，同步执行） ----------

#[derive(Deserialize)]
struct ExportCurToPathParams {
    path: String,
    request: crate::commands::ExportCurRequest,
}

#[derive(Deserialize)]
struct ExportAniToPathParams {
    path: String,
    request: crate::commands::ExportAniRequest,
}

#[derive(Deserialize)]
struct ImportFromPathParams {
    path: String,
}

#[derive(Deserialize)]
struct ImportFileParams {
    data: Vec<u8>,
}

fn handle_export_cur_to_path(id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: ExportCurToPathParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    write_export_response(id, &params.path, crate::commands::export_cur(params.request))
}

fn handle_export_ani_to_path(id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: ExportAniToPathParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    write_export_response(id, &params.path, crate::commands::export_ani(params.request))
}

/// 编码 → 创建父目录 → 落盘 → `{path, bytes}`。
fn write_export_response(
    id: RequestId,
    path: &str,
    encoded: Result<Vec<u8>, String>,
) -> RpcResponse {
    if path.trim().is_empty() {
        return invalid_params(id, "path 不能为空");
    }
    let bytes = match encoded {
        Ok(b) => b,
        Err(e) => return invalid_params(id, format!("编码失败: {e}")),
    };
    let out = PathBuf::from(path);
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return RpcResponse::error(
                    Some(id),
                    RpcError::new(codes::PLUGIN_INTERNAL, format!("创建输出目录失败: {e}")),
                );
            }
        }
    }
    let len = bytes.len();
    if let Err(e) = std::fs::write(&out, &bytes) {
        return RpcResponse::error(
            Some(id),
            RpcError::new(codes::PLUGIN_INTERNAL, format!("写入输出失败: {e}")),
        );
    }
    success(id, json!({ "path": path, "bytes": len }))
}

fn handle_import_from_path(id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: ImportFromPathParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    if params.path.trim().is_empty() {
        return invalid_params(id, "path 不能为空");
    }
    let input = PathBuf::from(&params.path);
    if !input.is_file() {
        return invalid_params(id, format!("输入文件不存在: {}", params.path));
    }
    match std::fs::read(&input) {
        Ok(data) => decode_import_response(id, data),
        Err(e) => RpcResponse::error(
            Some(id),
            RpcError::new(codes::PLUGIN_INTERNAL, format!("读取输入失败: {e}")),
        ),
    }
}

fn handle_import_file_bytes(id: RequestId, params: Option<Value>) -> RpcResponse {
    let params: ImportFileParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    decode_import_response(id, params.data)
}

/// 自动检测格式解码 → `{format, frames}`（与 `import_file` 命令返回同形）。
fn decode_import_response(id: RequestId, data: Vec<u8>) -> RpcResponse {
    match crate::commands::import_file(data) {
        Ok(result) => success(id, result),
        Err(e) => invalid_params(id, format!("解码失败: {e}")),
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
            eprintln!("[vine-cursor] task {} cancel requested", params.task_id);
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
