//! VineHub 宿主入口：Tauri 2 应用装配。

mod commands;

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::Manager;
use vine_host_core::{PluginRegistry, SidecarManager, Store};

/// 宿主全局状态。
pub struct AppState {
    pub registry: Mutex<PluginRegistry>,
    pub store: Mutex<Store>,
    pub sidecars: Mutex<SidecarManager>,
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

            app.manage(AppState {
                registry: Mutex::new(registry),
                store: Mutex::new(store),
                sidecars: Mutex::new(SidecarManager::new()),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running VineHub");
}
