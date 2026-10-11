pub mod archive;
pub mod commands;
pub mod detect;
pub mod dupes;
pub mod export;
pub mod fsops;
pub mod kindx;
pub mod manifest;
pub mod meta;
pub mod naming;
pub mod notes;
pub mod project;
pub mod rpc;
pub mod scan;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::JobState::default())
        .invoke_handler(tauri::generate_handler![
            commands::scan_directory,
            commands::probe_tools,
            commands::save_project,
            commands::load_project,
            commands::start_generate_notes,
            commands::start_export,
            commands::cancel_job,
            commands::auto_fill_proposals,
            commands::list_dir,
            commands::open_path,
            commands::reveal_path,
            commands::make_directory,
            commands::copy_items,
            commands::move_items,
            commands::rename_path,
            commands::name_profiles,
            commands::check_names,
            commands::fix_names,
            commands::preview_note,
            commands::note_diff,
            commands::cover_image,
            commands::start_note_audit,
            commands::start_find_duplicates,
            commands::start_verify,
            commands::preflight_export,
            commands::trash_items,
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running vinePress");
}
