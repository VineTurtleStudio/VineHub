//! VineHub 宿主入口：Tauri 2 应用装配。

mod commands;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::Manager;
use vine_host_core::{PluginRegistry, SidecarManager, Store};

/// 进行中的 sidecar 任务（每插件至多一个）。
pub struct TaskSlot {
    /// 后台线程确认 taskId 后写入。
    pub task_holder: Arc<Mutex<Option<String>>>,
    /// ack+终态双响应的终态槽。
    pub terminal: Arc<Mutex<Option<Result<Value, String>>>>,
    /// 最近一条 task.progress 通知（task_list 展示用）。
    pub last_progress: Arc<Mutex<Option<Value>>>,
    /// 发起时的 RPC 方法与参数快照（任务中心重试用）。
    pub method: String,
    pub params: Value,
}

/// 宿主全局状态。
pub struct AppState {
    pub registry: Mutex<PluginRegistry>,
    pub store: Mutex<Store>,
    pub sidecars: Mutex<SidecarManager>,
    /// key = plugin_id（每插件至多一个活动任务）。
    pub tasks: Mutex<HashMap<String, TaskSlot>>,
    /// sidecar stderr 日志落盘目录（`{data_dir}/logs`）。
    pub logs_dir: PathBuf,
}

/// 开发期仓库内 `plugins/` 目录（安装版不存在时回退 app data 目录）。
fn dev_plugins_dir() -> Option<PathBuf> {
    let manifest_dir = option_env!("CARGO_MANIFEST_DIR")?;
    let base = PathBuf::from(manifest_dir);
    let root = base.parent()?.parent()?.parent()?; // apps/vine-hub/src-tauri → 仓库根
    let plugins = root.join("plugins");
    plugins.is_dir().then_some(plugins)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("无法创建数据目录 {}: {e}", data_dir.display()))?;

            let store = Store::open(&data_dir.join("hub.db"))
                .map_err(|e| format!("打开配置库失败: {e}"))?;

            let plugins_dir = store
                .get_setting("plugins.dir")
                .ok()
                .flatten()
                .map(PathBuf::from)
                .or_else(dev_plugins_dir)
                .unwrap_or_else(|| data_dir.join("plugins"));

            let mut registry = PluginRegistry::new(&plugins_dir);
            registry
                .scan()
                .map_err(|e| format!("扫描插件目录失败: {e}"))?;

            let logs_dir = data_dir.join("logs");
            std::fs::create_dir_all(&logs_dir)
                .map_err(|e| format!("无法创建日志目录 {}: {e}", logs_dir.display()))?;

            // P3.1：asset protocol 放行插件目录（面板 iframe 加载 dist 资源）
            app.asset_protocol_scope()
                .allow_directory(&plugins_dir, true)
                .map_err(|e| format!("配置 asset scope 失败: {e}"))?;

            app.manage(AppState {
                registry: Mutex::new(registry),
                store: Mutex::new(store),
                sidecars: Mutex::new(SidecarManager::new()),
                tasks: Mutex::new(HashMap::new()),
                logs_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_plugins,
            commands::rescan_plugins,
            commands::launch_plugin,
            commands::set_entry_override,
            commands::set_plugin_enabled,
            commands::import_plugin_zip,
            commands::get_plugins_dir,
            commands::scan_start,
            commands::scan_poll,
            commands::scan_cancel,
            commands::scan_reset,
            commands::sidecar_status,
            commands::task_list,
            commands::task_cancel_id,
            commands::task_retry,
            commands::task_start,
            commands::resolve_panel_ui,
            commands::config_get,
            commands::config_set,
            commands::host_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running VineHub");
}
