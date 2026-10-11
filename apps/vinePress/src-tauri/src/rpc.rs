//! `--rpc` 模式：sidecar JSON-RPC over stdio（协议 v1，vinePress）。
//!
//! - argv 最早分流进入本模块，跳过 Tauri GUI 初始化
//! - 方法面：`plan.start` / `dupes.find` / `export.start`（zip）
//!   + P3.3 GUI 面覆盖（`scan.directory`/`project.*`/`fs.*`/`names.*`/`notes.*`/`tools.probe`）
//!   / `task.cancel`（通知）；ack 先回 taskId，进度走 `task.progress`，
//!   终态以同 id 第二响应返回（id 配对 = 终态）

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

use crate::{commands, detect, project, scan};

const PLUGIN_ID: &str = "vine.press";
const CAPABILITIES: &[&str] = &[
    "plan", "dupes", "export", "cancel", "scan", "project", "fs", "names", "notes", "tools",
];

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
            Err(e) => eprintln!("[vine-press] 编码消息失败: {}", e.message),
        }
    }

    fn send_notification(&self, method_name: &str, params: impl Serialize) {
        let mut note = RpcRequest::notification(method_name);
        match serde_json::to_value(params) {
            Ok(v) => note.params = Some(v),
            Err(e) => {
                eprintln!("[vine-press] 通知参数序列化失败: {e}");
                return;
            }
        }
        self.send_message(&note);
    }
}

/// RPC 模式主循环（stdin EOF 即退出）。
pub fn run() -> ExitCode {
    eprintln!(
        "[vine-press] rpc mode started (api {}, pid {})",
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
                eprintln!("[vine-press] 无法解析的请求行: {}", e.message);
                state.send_message(&RpcResponse::error(None, e.into_rpc_error()));
            }
        }
    }

    eprintln!("[vine-press] rpc stdin closed");

    // 等待在途任务回终态（最多 10s），避免宿主关闭输入时丢失终态
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let active = state.tasks.lock().expect("tasks lock").len();
        if active == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("[vine-press] 等待在途任务超时（{active} 个），强制退出");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    eprintln!("[vine-press] rpc mode exit");
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
                        eprintln!(
                            "[vine-press] task {} cancel requested (notification)",
                            params.task_id
                        );
                    }
                }
                Err(e) => eprintln!("[vine-press] task.cancel 参数非法: {e}"),
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
        method::PLAN_START => handle_plan_start(state, id, req.params),
        method::DUPES_FIND => handle_dupes_find(state, id, req.params),
        method::EXPORT_START => handle_export_start(state, id, req.params),
        // ---- P3.3 GUI 面覆盖 ----
        method::TOOLS_PROBE => handle_tools_probe(id, req.params),
        method::SCAN_DIRECTORY => handle_scan_directory(state, id, req.params),
        method::PROJECT_SAVE => handle_project_save(id, req.params),
        method::PROJECT_LOAD => handle_project_load(id, req.params),
        method::FS_LIST => handle_fs_list(id, req.params),
        method::FS_MKDIR => handle_fs_mkdir(id, req.params),
        method::NAMES_PROFILES => handle_names_profiles(id, req.params),
        method::NOTES_PREVIEW => handle_notes_preview(id, req.params),
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

fn spawn_task(
    state: &Arc<RpcState>,
) -> Result<(String, Arc<AtomicBool>), RpcError> {
    let task_id = format!("t-{}", state.next_task.fetch_add(1, Ordering::SeqCst) + 1);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tasks = state.tasks.lock().expect("tasks lock");
    if tasks.len() >= 2 {
        // 每插件默认最多 2 个并发任务（其余排队由宿主层调度）
        return Err(RpcError::new(
            codes::PLUGIN_INTERNAL,
            "并发任务已达上限（2），请稍后再试",
        ));
    }
    tasks.insert(task_id.clone(), TaskEntry { cancel: cancel.clone() });
    Ok((task_id, cancel))
}

#[derive(Deserialize)]
struct PlanStartParams {
    root: String,
}

fn handle_plan_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: PlanStartParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let root = PathBuf::from(&params.root);
    if !root.is_dir() {
        return invalid_params(id, format!("目录不存在: {}", params.root));
    }

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
                msg: Some("扫描目录树…".into()),
            },
        );

        let terminal = match scan::scan_tree(&root) {
            Ok(tree) => {
                state.send_notification(
                    method::TASK_PROGRESS,
                    TaskProgressParams {
                        task_id: task.clone(),
                        done: 1,
                        total: 2,
                        msg: Some("识别单位…".into()),
                    },
                );
                let mut detected = detect::detect(&tree, &project::Overrides::default());
                crate::kindx::enrich_cover_matches(&root, &mut detected);
                let cancelled = cancel.load(Ordering::SeqCst);
                json!({
                    "taskId": task,
                    "root": root.display().to_string(),
                    "containerCount": detected.container_count,
                    "unitCount": detected.units.len(),
                    "ignoredCount": detected.ignored_count,
                    "totalFiles": detected.total_files,
                    "totalSize": detected.total_size,
                    "rootLoose": detected.root_loose,
                    "cancelled": cancelled,
                    "elapsedMs": start.elapsed().as_millis() as u64,
                    "units": detected.units.iter().map(|u| json!({
                        "relPath": u.rel_path,
                        "name": u.name,
                        "kind": u.kind,
                        "hasNotes": u.has_notes,
                        "needsReview": u.needs_review,
                        "fileCount": u.stats.file_count,
                        "totalSize": u.stats.total_size,
                    })).collect::<Vec<_>>(),
                })
            }
            Err(e) => json!({
                "taskId": task,
                "cancelled": cancel.load(Ordering::SeqCst),
                "error": e,
            }),
        };
        state.send_message(&success(terminal_id, terminal));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vine-press] plan task {task_id} finished");
    });

    ack
}

#[derive(Deserialize)]
struct DupesFindParams {
    root: String,
}

fn handle_dupes_find(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: DupesFindParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let root = PathBuf::from(&params.root);
    if !root.is_dir() {
        return invalid_params(id, format!("目录不存在: {}", params.root));
    }

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
        let groups = crate::dupes::find_duplicates(&root, &cancel, |phase, index, total, rel| {
            // 每 200 个文件或阶段切换时上报一次
            if index % 200 == 0 || index == total {
                state.send_notification(
                    method::TASK_PROGRESS,
                    TaskProgressParams {
                        task_id: progress_task.clone(),
                        done: index as u64,
                        total: total as u64,
                        msg: Some(format!("{phase} {rel}")),
                    },
                );
            }
        });
        let cancelled = cancel.load(Ordering::SeqCst);
        let total_dup_files: usize = groups.iter().map(|g| g.files.len()).sum();
        state.send_message(&success(
            terminal_id,
            json!({
                "taskId": task,
                "groupCount": groups.len(),
                "duplicateFiles": total_dup_files,
                "cancelled": cancelled,
                "elapsedMs": start.elapsed().as_millis() as u64,
                "groups": groups,
            }),
        ));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vine-press] dupes task {task_id} finished ({} groups)", groups.len());
    });

    ack
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportStartParams {
    root: String,
    output_root: String,
    /// 只导出这些单位（rel path）；缺省 = 全部
    #[serde(default)]
    entry_rel_paths: Option<Vec<String>>,
    #[serde(default)]
    settings: Option<project::ArchiveSettings>,
}

fn handle_export_start(
    state: &Arc<RpcState>,
    id: RequestId,
    params: Option<Value>,
) -> RpcResponse {
    let params: ExportStartParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    let root = PathBuf::from(&params.root);
    if !root.is_dir() {
        return invalid_params(id, format!("目录不存在: {}", params.root));
    }
    let output_root = PathBuf::from(&params.output_root);
    if output_root.as_os_str().is_empty() {
        return invalid_params(id, "outputRoot 不能为空");
    }
    // RPC 模式缺省 = zip 安全设置（独立 GUI 默认仍是 rar + 加密 + 注释）
    let settings = params.settings.unwrap_or_else(|| project::ArchiveSettings {
        format: "zip".into(),
        volume_mib: 0,
        comment: false,
        encrypt_names: false,
        overwrite: true,
        write_notes: false,
        incremental: false,
    });
    if settings.format != "zip" {
        return invalid_params(id, "RPC 模式暂仅支持 zip 导出（RAR 需 WinRAR，留独立 GUI）");
    }

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
                msg: Some("扫描并识别…".into()),
            },
        );

        let terminal = (|| {
            let tree = scan::scan_tree(&root)?;
            let mut detected = detect::detect(&tree, &project::Overrides::default());
            crate::kindx::enrich_cover_matches(&root, &mut detected);
            let mut entries: Vec<project::Entry> =
                detected.units.iter().map(project::Entry::from_unit).collect();
            if let Some(filter) = &params.entry_rel_paths {
                let set: std::collections::HashSet<&str> =
                    filter.iter().map(|s| s.as_str()).collect();
                entries.retain(|e| set.contains(e.rel_path.as_str()));
            }
            if entries.is_empty() {
                return Err("没有可导出的单位（检查路径或过滤条件）".to_string());
            }

            let cfg = crate::export::ExportConfig {
                rar_path: "",
                root: &root,
                output_root: &output_root,
                settings: &settings,
                profiles: &[],
                entries: &entries,
            };
            let final_progress = crate::export::run_export(&cfg, &|p| {
                state.send_notification(
                    method::TASK_PROGRESS,
                    TaskProgressParams {
                        task_id: progress_task.clone(),
                        done: p.global_percent as u64,
                        total: 100,
                        msg: Some(format!("{} {}/{} {}", p.phase, p.index, p.total, p.rel_path)),
                    },
                );
            }, &cancel);
            Ok::<_, String>(json!({
                "taskId": task,
                "phase": final_progress.phase,
                "status": final_progress.status,
                "message": final_progress.message,
                "created": final_progress.created,
                "skipped": final_progress.skipped,
                "failed": final_progress.failed,
                "archives": final_progress.archives,
                "cancelled": cancel.load(Ordering::SeqCst),
                "elapsedMs": start.elapsed().as_millis() as u64,
            }))
        })();

        let payload = match terminal {
            Ok(v) => v,
            Err(e) => json!({
                "taskId": task,
                "status": "error",
                "error": e,
                "cancelled": cancel.load(Ordering::SeqCst),
            }),
        };
        state.send_message(&success(terminal_id, payload));
        state.tasks.lock().expect("tasks lock").remove(&task_id);
        eprintln!("[vine-press] export task {task_id} finished");
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
            eprintln!("[vine-press] task {} cancel requested", params.task_id);
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

// ==================== P3.3 GUI 面覆盖（快速单响应） ====================

fn handle_tools_probe(id: RequestId, _params: Option<Value>) -> RpcResponse {
    success(id, commands::probe_tools())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectPathParams {
    path: String,
}

fn handle_project_save(id: RequestId, params: Option<Value>) -> RpcResponse {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        path: String,
        project: project::Project,
    }
    let p: Params = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    match commands::save_project(p.path, p.project) {
        Ok(()) => success(id, json!({ "ok": true })),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}

fn handle_project_load(id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: ProjectPathParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    match commands::load_project(p.path) {
        Ok(project) => success(id, project),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FsListParams {
    root: String,
    path: String,
    #[serde(default)]
    show_all: bool,
}

fn handle_fs_list(id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: FsListParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    match commands::list_dir(p.root, p.path, p.show_all) {
        Ok(items) => success(id, items),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FsMkdirParams {
    root: String,
    path: String,
}

fn handle_fs_mkdir(id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: FsMkdirParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    match commands::make_directory(p.root, p.path) {
        Ok(()) => success(id, json!({ "ok": true })),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}

fn handle_names_profiles(id: RequestId, _params: Option<Value>) -> RpcResponse {
    success(id, commands::name_profiles())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesPreviewParams {
    root: String,
    entry: project::Entry,
}

fn handle_notes_preview(id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: NotesPreviewParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    match commands::preview_note(p.root, p.entry) {
        Ok(text) => success(id, json!({ "text": text })),
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}

// ==================== P3.3 全保真扫描（阻塞单响应，对齐 GUI invoke 语义） ====================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanDirectoryParams {
    root: String,
    #[serde(default)]
    overrides: project::Overrides,
}

fn handle_scan_directory(_state: &Arc<RpcState>, id: RequestId, params: Option<Value>) -> RpcResponse {
    let p: ScanDirectoryParams = match serde_json::from_value(params.unwrap_or(Value::Null)) {
        Ok(p) => p,
        Err(e) => return invalid_params(id, format!("参数非法: {e}")),
    };
    if !PathBuf::from(&p.root).is_dir() {
        return invalid_params(id, format!("root 不是目录: {}", p.root));
    }
    eprintln!("[vine-press] scan.directory start: {}", p.root);
    match commands::scan_directory(p.root, p.overrides) {
        Ok(resp) => {
            eprintln!("[vine-press] scan.directory done");
            success(id, resp)
        }
        Err(e) => RpcResponse::error(Some(id), RpcError::new(codes::PLUGIN_INTERNAL, e)),
    }
}
