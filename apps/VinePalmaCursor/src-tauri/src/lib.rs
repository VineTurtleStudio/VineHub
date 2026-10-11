pub mod codec;
pub mod commands;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            commands::export_cur,
            commands::export_ani,
            commands::import_cursor,
            commands::import_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running VinePalmaCursor");
}