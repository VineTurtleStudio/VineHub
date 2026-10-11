//! 宿主命令面（React 通过 invoke 调用）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tauri::State;
use vine_host_core::sidecar::{SidecarConfig, SidecarHandle};
use vine_host_core::{PluginStatus, StorageError};
use vine_plugin_api::{PluginManifest, method};

use crate::AppState;

/// CREATE_NO_WINDOW：外部程序启动不弹控制台。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 前端可见的插件视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub kind: String,
    pub enabled: bool,
    /// 是否可启动（已启用且解析得到入口）。
    pub available: bool,
    pub status_reason: Option<String>,
    pub entry_override: Option<String>,
    pub resolved_entry: Option<String>,
}

/// 导入 zip 的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub ok: bool,
    pub message: String,
    pub plugin_id: Option<String>,
}

fn build_views(state: &AppState) -> Vec<PluginView> {
    let registry = state.registry.lock().unwrap();
    let store = state.store.lock().unwrap();

    let mut views: Vec<PluginView> = registry
        .list()
        .map(|entry| {
            let record = store.get_plugin(&entry.key).ok().flatten();
            let enabled = record.as_ref().map(|r| r.enabled).unwrap_or(true);
            let entry_override = record.and_then(|r| r.entry_override);

            let (name, version, api_version, kind, registry_reason) = match &entry.manifest {
                Some(m) => {
                    let reason = match &entry.status {
                        PluginStatus::Available => None,
                        PluginStatus::Unavailable { reason } => Some(reason.clone()),
                    };
                    (
                        m.name.clone(),
                        m.version.clone(),
                        m.api_version.clone(),
                        format!("{:?}", m.kind).to_lowercase(),
                        reason,
                    )
                }
                None => {
                    let reason = match &entry.status {
                        PluginStatus::Unavailable { reason } => reason.clone(),
                        _ => "未知错误".to_string(),
                    };
                    (
                        entry.key.clone(),
                        "0.0.0".to_string(),
                        "1.0".to_string(),
                        "external".to_string(),
                        Some(reason),
                    )
                }
            };

            let resolved_entry = registry
                .resolve_entry(&entry.key, entry_override.as_deref().map(Path::new))
                .map(|p| p.display().to_string());

            let available = enabled && resolved_entry.is_some();
            let status_reason = if !enabled {
                Some("已停用".to_string())
            } else if !available {
                registry_reason.or_else(|| Some("未找到可执行文件".to_string()))
            } else {
                None
            };

            PluginView {
                id: entry.key.clone(),
                name,
                version,
                api_version,
                kind,
                enabled,
                available,
                status_reason,
                entry_override,
                resolved_entry,
            }
        })
        .collect();

    views.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    views
}

#[tauri::command]
pub fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginView>, String> {
    Ok(build_views(&state))
}

#[tauri::command]
pub fn rescan_plugins(state: State<'_, AppState>) -> Result<Vec<PluginView>, String> {
    {
        let mut registry = state.registry.lock().unwrap();
        registry.scan().map_err(|e| e.to_string())?;
    }
    Ok(build_views(&state))
}

/// 一键启动独立程序（external 生命周期）。
#[tauri::command]
pub fn launch_plugin(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let exe: PathBuf = {
        let registry = state.registry.lock().unwrap();
        let store = state.store.lock().unwrap();
        let record = store.get_plugin(&id).map_err(storage_err)?;
        let entry_override = record.and_then(|r| r.entry_override).map(PathBuf::from);
        registry
            .resolve_entry(&id, entry_override.as_deref())
            .ok_or_else(|| format!("找不到可执行文件：{id}（可在插件详情页配置覆盖路径）"))?
    };

    let mut command = Command::new(&exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map_err(|e| format!("启动失败 {}: {e}", exe.display()))?;
    Ok(())
}

/// 设置 / 清除 exe 路径覆盖。
#[tauri::command]
pub fn set_entry_override(
    id: String,
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<PluginView>, String> {
    {
        let store = state.store.lock().unwrap();
        let clean = path.as_deref().filter(|p| !p.trim().is_empty());
        // 确保插件已有登记行；没有则按当前 manifest 生成一条
        if store.get_plugin(&id).map_err(storage_err)?.is_none() {
            let registry = state.registry.lock().unwrap();
            if let Some(entry) = registry.get(&id) {
                if let Some(m) = &entry.manifest {
                    store
                        .upsert_plugin(&vine_host_core::PluginRecord {
                            id: id.clone(),
                            name: m.name.clone(),
                            version: m.version.clone(),
                            api_version: m.api_version.clone(),
                            kind: format!("{:?}", m.kind).to_lowercase(),
                            enabled: true,
                            entry_override: None,
                            data_dir: m.data_dir.clone(),
                            installed_at: now_unix(),
                        })
                        .map_err(storage_err)?;
                }
            }
        }
        store
            .set_entry_override(&id, clean)
            .map_err(storage_err)?;
    }
    Ok(build_views(&state))
}

/// 启用 / 停用插件。
#[tauri::command]
pub fn set_plugin_enabled(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<Vec<PluginView>, String> {
    {
        let store = state.store.lock().unwrap();
        if store.get_plugin(&id).map_err(storage_err)?.is_none() {
            return Err(format!("插件未登记: {id}"));
        }
        store.set_plugin_enabled(&id, enabled).map_err(storage_err)?;
    }
    Ok(build_views(&state))
}

#[tauri::command]
pub fn get_plugins_dir(state: State<'_, AppState>) -> Result<String, String> {
    let registry = state.registry.lock().unwrap();
    Ok(registry.plugins_dir().display().to_string())
}

/// 导入 `.vpkg`（zip）：解压 → 校验 manifest → 覆盖更新 → 重扫。
#[tauri::command]
pub fn import_plugin_zip(
    zip_path: String,
    state: State<'_, AppState>,
) -> Result<ImportResult, String> {
    let plugins_dir = {
        let registry = state.registry.lock().unwrap();
        registry.plugins_dir().to_path_buf()
    };
    std::fs::create_dir_all(&plugins_dir)
        .map_err(|e| format!("无法创建插件目录 {}: {e}", plugins_dir.display()))?;

    let file = std::fs::File::open(&zip_path)
        .map_err(|e| format!("无法打开包文件: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("不是合法的 zip 包: {e}"))?;

    // 找到包内 plugin.json 并校验
    let manifest_idx = (0..archive.len())
        .find(|&i| {
            archive
                .by_index(i)
                .ok()
                .and_then(|f| f.enclosed_name())
                .map(|name| {
                    name.file_name()
                        .and_then(|n| n.to_str())
                        == Some(vine_plugin_api::PLUGIN_MANIFEST_FILENAME)
                })
                .unwrap_or(false)
        })
        .ok_or_else(|| "包内未找到 plugin.json".to_string())?;

    let manifest_json = {
        let mut manifest_file = archive
            .by_index(manifest_idx)
            .map_err(|e| e.to_string())?;
        let mut json = String::new();
        manifest_file
            .read_to_string(&mut json)
            .map_err(|e| e.to_string())?;
        json
    };
    let manifest = PluginManifest::from_json(&manifest_json)
        .map_err(|e| format!("manifest 校验失败: {e}"))?;

    let target = plugins_dir.join(&manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("清理旧版本失败: {e}"))?;
    }

    // 解压（enclosed_name 已防 zip-slip 越界）
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = file.enclosed_name() else {
            continue;
        };
        let out = target.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut outfile = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
        }
    }

    {
        let mut registry = state.registry.lock().unwrap();
        registry.scan().map_err(|e| e.to_string())?;
    }

    Ok(ImportResult {
        ok: true,
        message: format!("已导入 {} v{}", manifest.name, manifest.version),
        plugin_id: Some(manifest.id),
    })
}

fn storage_err(e: StorageError) -> String {
    e.to_string()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------- sidecar 扫描（P1 vinePulse） ----------

/// `scan_start` 返回值。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStartView {
    pub started: bool,
}

/// `scan_poll` 返回值：进度通知增量 + 终态/错误 + sidecar 状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPollView {
    pub notifications: Vec<Value>,
    pub task_id: Option<String>,
    pub terminal: Option<Value>,
    pub error: Option<String>,
    pub running: bool,
    pub sidecar_running: bool,
}

/// 确保目标插件的 sidecar 已连接（未运行则拉起并握手）。
fn ensure_sidecar(state: &crate::AppState, plugin_id: &str) -> Result<Arc<SidecarHandle>, String> {
    let mut sidecars = state.sidecars.lock().unwrap();
    if let Some(handle) = sidecars.get(plugin_id) {
        if handle.is_running() {
            return Ok(handle);
        }
        sidecars.kill(plugin_id);
    }

    let (exe, api_version) = {
        let registry = state.registry.lock().unwrap();
        let store = state.store.lock().unwrap();
        let record = store.get_plugin(plugin_id).map_err(storage_err)?;
        let entry_override = record.and_then(|r| r.entry_override).map(PathBuf::from);
        let exe = registry
            .resolve_entry(plugin_id, entry_override.as_deref())
            .ok_or_else(|| {
                format!("找不到可执行文件：{plugin_id}（可在插件详情页配置覆盖路径）")
            })?;
        let api_version = registry
            .get(plugin_id)
            .and_then(|e| e.manifest.as_ref())
            .map(|m| m.api_version.clone())
            .unwrap_or_else(|| "1.0".to_string());
        (exe, api_version)
    };

    let mut config = SidecarConfig::new(plugin_id, exe);
    config.args = vec!["--rpc".to_string()];
    config.expected_api_version = api_version;
    config.handshake_timeout = Duration::from_secs(5);
    config.request_timeout = Duration::from_secs(30);
    config.log_dir = Some(state.logs_dir.clone());
    sidecars.spawn(config).map_err(|e| format!("拉起 sidecar 失败: {e}"))
}

/// 毫秒级 Unix 时间戳。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 生成任务记录 id（宿主侧，与插件内部 taskId 区分）。
fn new_record_id() -> String {
    format!("task-{}-{:04x}", now_ms(), std::process::id() & 0xffff)
}

/// 通用任务启动：ensure sidecar → 能力校验 → ack+终态线程 → 登记内存槽与 DB 记录。
/// 返回宿主记录 id。
fn start_task_impl(
    state: &AppState,
    plugin_id: &str,
    method_name: &str,
    params: Value,
    require_cap: Option<&str>,
) -> Result<String, String> {
    {
        let mut tasks = state.tasks.lock().unwrap();
        if let Some(slot) = tasks.get(plugin_id) {
            let done = slot.terminal.lock().unwrap().is_some();
            if !done {
                return Err("该插件已有任务进行中（可先取消）".to_string());
            }
            tasks.remove(plugin_id);
        }
    }

    let handle = ensure_sidecar(state, plugin_id)?;
    if let Some(cap) = require_cap {
        if !handle.info.capabilities.iter().any(|c| c == cap) {
            return Err(format!("插件 {plugin_id} 不支持 {cap} 能力"));
        }
    }

    let record_id = new_record_id();
    let task_holder = Arc::new(Mutex::new(None::<String>));
    let terminal = Arc::new(Mutex::new(None::<Result<Value, String>>));
    let last_progress = Arc::new(Mutex::new(None::<Value>));

    {
        let task_holder = task_holder.clone();
        let terminal = terminal.clone();
        let method_name = method_name.to_string();
        let thread_params = params.clone();
        std::thread::spawn(move || {
            let result = handle.request_with_ack(
                &method_name,
                Some(thread_params),
                Duration::from_secs(10),
                Duration::from_secs(60 * 60),
            );
            match result {
                Ok((ack, done)) => {
                    if let Some(tid) = ack.get("taskId").and_then(|v| v.as_str()) {
                        *task_holder.lock().unwrap() = Some(tid.to_string());
                    }
                    *terminal.lock().unwrap() = Some(Ok(done));
                }
                Err(e) => {
                    *terminal.lock().unwrap() = Some(Err(e.to_string()));
                }
            }
        });
    }

    state.tasks.lock().unwrap().insert(
        plugin_id.to_string(),
        crate::TaskSlot {
            task_holder,
            terminal,
            last_progress,
            method: method_name.to_string(),
            params: params.clone(),
        },
    );

    let detail = json!({ "params": params }).to_string();
    let record = vine_host_core::TaskRecord {
        id: record_id.clone(),
        plugin_id: plugin_id.to_string(),
        method: method_name.to_string(),
        status: vine_host_core::TaskStatus::Running,
        created_at: now_ms(),
        updated_at: now_ms(),
        detail: Some(detail),
    };
    {
        let store = state.store.lock().unwrap();
        store.insert_task(&record).map_err(storage_err)?;
    }
    Ok(record_id)
}

/// 由终态槽推导任务状态与摘要。
fn derive_status(slot: &crate::TaskSlot) -> (vine_host_core::TaskStatus, String) {
    match slot.terminal.lock().unwrap().as_ref() {
        Some(Ok(v)) => {
            let cancelled = v.get("cancelled").and_then(|c| c.as_bool()).unwrap_or(false);
            let elapsed = v.get("elapsedMs").and_then(|e| e.as_u64()).unwrap_or(0);
            if cancelled {
                (vine_host_core::TaskStatus::Cancelled, format!("已取消（{elapsed} ms）"))
            } else {
                (vine_host_core::TaskStatus::Done, format!("完成（{elapsed} ms）"))
            }
        }
        Some(Err(e)) => {
            if e.contains("已取消") {
                (vine_host_core::TaskStatus::Cancelled, "已取消".to_string())
            } else {
                (vine_host_core::TaskStatus::Failed, e.clone())
            }
        }
        None => (vine_host_core::TaskStatus::Running, "进行中".to_string()),
    }
}

/// 前端可见的任务记录视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub plugin_id: String,
    pub method: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 摘要/错误信息。
    pub detail: Option<String>,
    /// 进行中任务的最新进度（done/total/msg）。
    pub progress: Option<Value>,
    /// 插件内部 taskId。
    pub plugin_task_id: Option<String>,
}

/// 任务列表（含终态对账：内存终态→DB；宿主重启后的悬挂 running→failed）。
#[tauri::command]
pub fn task_list(
    plugin_id: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<Vec<TaskView>, String> {
    let records = {
        let store = state.store.lock().unwrap();
        store
            .list_tasks(plugin_id.as_deref())
            .map_err(storage_err)?
    };

    let mut tasks = state.tasks.lock().unwrap();
    let mut views = Vec::with_capacity(records.len());
    for mut rec in records {
        if rec.status == vine_host_core::TaskStatus::Running {
            if let Some(slot) = tasks.get(&rec.plugin_id) {
                let (status, summary) = derive_status(slot);
                if status != vine_host_core::TaskStatus::Running {
                    let detail = json!({ "summary": summary }).to_string();
                    {
                        let store = state.store.lock().unwrap();
                        let _ = store.update_task(&rec.id, status, now_ms(), Some(&detail));
                    }
                    rec.status = status;
                    rec.detail = Some(detail);
                    rec.updated_at = now_ms();
                    tasks.remove(&rec.plugin_id);
                }
            } else {
                // 内存槽不存在（宿主重启或未启动过）：标记失败，可重试
                let detail = "{ \"summary\": \"宿主重启，任务丢失，可重试\" }".to_string();
                {
                    let store = state.store.lock().unwrap();
                    let _ = store.update_task(
                        &rec.id,
                        vine_host_core::TaskStatus::Failed,
                        now_ms(),
                        Some(&detail),
                    );
                }
                rec.status = vine_host_core::TaskStatus::Failed;
                rec.detail = Some(detail);
                rec.updated_at = now_ms();
            }
        }
        let (progress, plugin_task_id) = match tasks.get(&rec.plugin_id) {
            Some(slot) if rec.status == vine_host_core::TaskStatus::Running => {
                let tid = slot.task_holder.lock().unwrap().clone();
                let p = slot.last_progress.lock().unwrap().clone();
                (p, tid)
            }
            _ => (None, None),
        };
        views.push(TaskView {
            id: rec.id,
            plugin_id: rec.plugin_id,
            method: rec.method,
            status: rec.status.as_str().to_string(),
            created_at: rec.created_at,
            updated_at: rec.updated_at,
            detail: rec.detail,
            progress,
            plugin_task_id,
        });
    }
    Ok(views)
}

/// 按宿主记录 id 取消任务。
#[tauri::command]
pub fn task_cancel_id(
    task_id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let record = {
        let store = state.store.lock().unwrap();
        store.get_task(&task_id).map_err(storage_err)?
    }
    .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    let tasks = state.tasks.lock().unwrap();
    let Some(slot) = tasks.get(&record.plugin_id) else {
        return Err("任务已不在运行".to_string());
    };
    let Some(plugin_task_id) = slot.task_holder.lock().unwrap().clone() else {
        return Err("任务尚未确认，请稍后再试".to_string());
    };
    drop(tasks);

    let sidecars = state.sidecars.lock().unwrap();
    let handle = sidecars
        .get(&record.plugin_id)
        .ok_or_else(|| "sidecar 未运行".to_string())?;
    handle
        .notify(
            method::TASK_CANCEL,
            Some(json!({ "taskId": plugin_task_id })),
        )
        .map_err(|e| format!("发送取消失败: {e}"))
}

/// 重试任务：读取记录中的方法与参数快照，重新启动；返回新记录 id。
#[tauri::command]
pub fn task_retry(
    task_id: String,
    state: State<'_, crate::AppState>,
) -> Result<String, String> {
    let record = {
        let store = state.store.lock().unwrap();
        store.get_task(&task_id).map_err(storage_err)?
    }
    .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    if record.status == vine_host_core::TaskStatus::Running {
        return Err("任务仍在进行中，无需重试".to_string());
    }

    let params = record
        .detail
        .as_deref()
        .and_then(|d| serde_json::from_str::<Value>(d).ok())
        .and_then(|v| v.get("params").cloned())
        .unwrap_or(Value::Null);
    if params.is_null() {
        return Err("该任务缺少参数快照，无法重试".to_string());
    }

    let new_id = start_task_impl(&state, &record.plugin_id, &record.method, params, None)?;
    Ok(new_id)
}

/// 启动扫描（ack+终态双响应；taskId 由 poll 返回）。
#[tauri::command]
pub fn scan_start(
    plugin_id: String,
    path: String,
    types: Vec<String>,
    deep: bool,
    state: State<'_, crate::AppState>,
) -> Result<ScanStartView, String> {
    if path.trim().is_empty() {
        return Err("请先选择扫描路径".to_string());
    }
    if !Path::new(&path).exists() {
        return Err(format!("路径不存在: {path}"));
    }
    let params = json!({ "path": path, "types": types, "deep": deep });
    start_task_impl(&state, &plugin_id, method::SCAN_START, params, Some("scan"))?;
    Ok(ScanStartView { started: true })
}

/// 轮询扫描进度（通知 + 终态 + sidecar 存活）。
#[tauri::command]
pub fn scan_poll(
    plugin_id: String,
    state: State<'_, crate::AppState>,
) -> Result<ScanPollView, String> {
    let mut notifications = Vec::new();
    let sidecar_running = {
        let sidecars = state.sidecars.lock().unwrap();
        match sidecars.get(&plugin_id) {
            Some(handle) => {
                while let Some(note) = handle.try_recv_notification() {
                    if note.method == method::TASK_PROGRESS {
                        if let Some(params) = note.params {
                            notifications.push(params);
                        }
                    }
                }
                handle.is_running()
            }
            None => false,
        }
    };

    let tasks = state.tasks.lock().unwrap();
    let Some(slot) = tasks.get(&plugin_id) else {
        return Ok(ScanPollView {
            notifications,
            task_id: None,
            terminal: None,
            error: None,
            running: false,
            sidecar_running,
        });
    };

    if let Some(last) = notifications.last() {
        *slot.last_progress.lock().unwrap() = Some(last.clone());
    }

    let task_id = slot.task_holder.lock().unwrap().clone();
    let (terminal, error) = match slot.terminal.lock().unwrap().as_ref() {
        Some(Ok(v)) => (Some(v.clone()), None),
        Some(Err(e)) => (None, Some(e.clone())),
        None => (None, None),
    };
    let running = terminal.is_none() && error.is_none();
    Ok(ScanPollView {
        notifications,
        task_id,
        terminal,
        error,
        running,
        sidecar_running,
    })
}

/// 取消当前扫描（task.cancel 通知）。
#[tauri::command]
pub fn scan_cancel(
    plugin_id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let task_id = {
        let tasks = state.tasks.lock().unwrap();
        let Some(slot) = tasks.get(&plugin_id) else {
            return Err("没有进行中的扫描".to_string());
        };
        let Some(task_id) = slot.task_holder.lock().unwrap().clone() else {
            return Err("任务尚未确认，请稍后再试".to_string());
        };
        task_id
    };

    let sidecars = state.sidecars.lock().unwrap();
    let handle = sidecars
        .get(&plugin_id)
        .ok_or_else(|| "sidecar 未运行".to_string())?;
    handle
        .notify(
            method::TASK_CANCEL,
            Some(json!({ "taskId": task_id })),
        )
        .map_err(|e| format!("发送取消失败: {e}"))
}

/// 清除扫描终态（开始新一轮 / 重试前调用）。
#[tauri::command]
pub fn scan_reset(
    plugin_id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    state.tasks.lock().unwrap().remove(&plugin_id);
    // 顺带回收通知残留
    if let Some(handle) = state.sidecars.lock().unwrap().get(&plugin_id) {
        while handle.try_recv_notification().is_some() {}
    }
    Ok(())
}

/// sidecar 状态（含握手信息），供详情页诊断。
#[tauri::command]
pub fn sidecar_status(
    plugin_id: String,
    state: State<'_, crate::AppState>,
) -> Result<Value, String> {
    let sidecars = state.sidecars.lock().unwrap();
    match sidecars.get(&plugin_id) {
        Some(handle) if handle.is_running() => Ok(json!({
            "running": true,
            "id": handle.info.id,
            "version": handle.info.version,
            "apiVersion": handle.info.api_version,
            "capabilities": handle.info.capabilities,
        })),
        Some(_) => Ok(json!({ "running": false, "reason": "进程已退出（可重试自动拉起）" })),
        None => Ok(json!({ "running": false, "reason": "尚未连接" })),
    }
}
