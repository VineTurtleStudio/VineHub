mod commands;
mod core;
mod msg;
mod model;
mod runtime;
mod storage;
mod trigger;

use std::sync::Mutex;
use storage::db::Db;
use tauri::Manager;

/// 应用共享状态
pub struct AppCtx {
    pub config_path: std::path::PathBuf,
    /// 数据目录（便携模式为 `<exe>/portable_data`，否则为系统用户目录）
    pub data_dir: std::path::PathBuf,
    pub portable: bool,
    pub config: Mutex<model::Config>,
    pub db: Mutex<Db>,
    pub syncer: runtime::Syncer,
    pub watch: trigger::WatchState,
    pub trigger: trigger::TriggerState,
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::with_id(app, "show", msg::tray_show(), true, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", msg::tray_sync(), true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", msg::tray_quit(), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &sync, &sep, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("缺少应用图标（icons/icon.png）");

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("vineSync")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "sync" => runtime::request_sync(app, Vec::new(), "manual"),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

pub fn run() {
    // 便携模式：数据目录跟随程序目录（须在创建 WebView2 之前设置用户数据目录）
    let portable_dir = storage::paths::portable_data_dir();
    if let Some(dir) = &portable_dir {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir.join("webview2"));
    }

    tauri::Builder::default()
        // 单实例：第二个进程激活已有窗口后自行退出
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(move |app| {
            let app_dir = match &portable_dir {
                Some(dir) => dir.clone(),
                None => app.path().app_config_dir().map_err(io_err)?,
            };
            std::fs::create_dir_all(&app_dir).map_err(io_err)?;
            let config_path = app_dir.join("config.json");
            let config = storage::config::load(&config_path);
            msg::set(config.language);
            // 启动时与配置保持一致（disable 对未注册场景是幂等空操作）
            commands::apply_autostart(app.handle(), config.autostart);
            let db = Db::open(&app_dir.join("state.db")).map_err(io_err)?;
            let syncer = runtime::Syncer::spawn(app.handle().clone());

            app.manage(AppCtx {
                config_path,
                data_dir: app_dir,
                portable: portable_dir.is_some(),
                config: Mutex::new(config),
                db: Mutex::new(db),
                syncer,
                watch: trigger::WatchState::default(),
                trigger: trigger::TriggerState::default(),
            });

            build_tray(app.handle())?;
            trigger::start_trigger_thread(app.handle().clone());
            trigger::refresh_watchers(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_app_info,
            commands::save_config,
            commands::preview_sync,
            commands::start_sync,
            commands::get_history,
            commands::clear_history,
            commands::cleanup_versions,
            commands::test_filter,
            commands::export_history,
            commands::set_sync_paused,
        ])
        .run(tauri::generate_context!())
        .expect("启动 vineSync 失败");
}
