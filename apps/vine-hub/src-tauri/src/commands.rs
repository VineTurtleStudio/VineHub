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
    sidecars.spawn(config).map_err(|e| format!("拉起 sidecar 失败: {e}"))
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
    {
        let mut scans = state.pulse_scans.lock().unwrap();
        if let Some(scan) = scans.get(&plugin_id) {
            let done = scan.terminal.lock().unwrap().is_some();
            if !done {
                return Err("已有扫描任务进行中（可先取消）".to_string());
            }
            scans.remove(&plugin_id);
        }
    }

    let handle = ensure_sidecar(&state, &plugin_id)?;
    if !handle.info.capabilities.iter().any(|c| c == "scan") {
        return Err(format!("插件 {} 不支持 scan 能力", plugin_id));
    }

    let task_holder = Arc::new(Mutex::new(None::<String>));
    let terminal = Arc::new(Mutex::new(None::<Result<Value, String>>));

    {
        let task_holder = task_holder.clone();
        let terminal = terminal.clone();
        let params = json!({ "path": path, "types": types, "deep": deep });
        std::thread::spawn(move || {
            let result = handle.request_with_ack(
                method::SCAN_START,
                Some(params),
                Duration::from_secs(10),
                Duration::from_secs(30 * 60),
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

    state.pulse_scans.lock().unwrap().insert(
        plugin_id,
        crate::PulseScan {
            task_holder,
            terminal,
        },
    );
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

    let scans = state.pulse_scans.lock().unwrap();
    let Some(scan) = scans.get(&plugin_id) else {
        return Ok(ScanPollView {
            notifications,
            task_id: None,
            terminal: None,
            error: None,
            running: false,
            sidecar_running,
        });
    };

    let task_id = scan.task_holder.lock().unwrap().clone();
    let (terminal, error) = match scan.terminal.lock().unwrap().as_ref() {
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
        let scans = state.pulse_scans.lock().unwrap();
        let Some(scan) = scans.get(&plugin_id) else {
            return Err("没有进行中的扫描".to_string());
        };
        let Some(task_id) = scan.task_holder.lock().unwrap().clone() else {
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
    state.pulse_scans.lock().unwrap().remove(&plugin_id);
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
