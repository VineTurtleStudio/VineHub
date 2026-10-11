pub mod audit;
pub mod auth;
pub mod autocat;
pub mod backup;
pub mod browse;
pub mod categories;
pub mod commands;
pub mod db;
pub mod dupes;
pub mod env;
pub mod fts;
pub mod meta;
pub mod ocr;
pub mod port;
pub mod rules;
pub mod scan;
pub mod shell;
pub mod smart;
pub mod tags;
pub mod tasks;
pub mod thumbs;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app
                .path()
                .app_local_data_dir()
                .map_err(|e| format!("无法解析 AppData 目录: {e}"))?;
            let db_path = dir.join("catalog.sqlite");
            let pool = db::open_pool(&db_path)
                .map_err(|e| format!("打开数据库失败 {}: {e}", db_path.display()))?;
            let applied = {
                let mut conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
                db::migrate::migrate(&mut conn).map_err(|e| format!("数据库迁移失败: {e}"))?
            };
            // FTS5 一致性检查（条目数 ≠ 索引行数 → 全量重建，无 trigger 手动同步）
            {
                let mut conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
                match fts::ensure_consistent(&mut conn) {
                    Ok(true) => eprintln!("[vineCurator] fts_items 已全量重建"),
                    Ok(false) => {}
                    Err(e) => eprintln!("[vineCurator] fts 一致性检查失败: {e}"),
                }
            }
            // 示例智能集合：仅首次播种（settings 标记，幂等）
            {
                let conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
                if let Err(e) = smart::seed_example_collections(&conn) {
                    eprintln!("[vineCurator] 示例集合播种失败: {e}");
                }
            }
            // 缩略图目录：加入 asset 协议白名单（前端 convertFileSrc 直接 <img>）
            {
                let conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
                match thumbs::derived_root(&conn, &dir.join("derived")) {
                    Ok(thumbs_dir) => {
                        if let Err(e) = app
                            .asset_protocol_scope()
                            .allow_directory(&thumbs_dir, true)
                        {
                            eprintln!("[vineCurator] 缩略图目录 asset 白名单失败: {e}");
                        }
                    }
                    Err(e) => eprintln!("[vineCurator] 缩略图目录初始化失败: {e}"),
                }
            }
            // 根目录：加入 asset 协议白名单（灯箱预览原图/原视频，只读）
            {
                let conn = pool.get().map_err(|e| format!("获取数据库连接失败: {e}"))?;
                match db::repo::list_roots(&conn) {
                    Ok(roots) => {
                        for r in roots {
                            if let Err(e) =
                                app.asset_protocol_scope().allow_directory(&r.path, true)
                            {
                                eprintln!("[vineCurator] 根目录 asset 白名单失败 {}: {e}", r.path);
                            }
                        }
                    }
                    Err(e) => eprintln!("[vineCurator] 根目录列表读取失败: {e}"),
                }
            }
            let db_display = db_path.display().to_string();
            app.manage(db::DbState { pool, db_path });
            app.manage(commands::JobState::default());
            app.manage(auth::AuthState::default());
            eprintln!(
                "[vineCurator] db={} migrations_applied={:?}",
                db_display, applied
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::probe_tools,
            commands::get_db_info,
            commands::get_settings,
            commands::save_settings,
            commands::list_roots,
            commands::add_root,
            commands::remove_root,
            commands::redirect_root,
            commands::start_scan,
            commands::start_hash_job,
            commands::cancel_job,
            commands::start_meta_job,
            commands::list_items,
            commands::list_categories,
            commands::get_overview,
            commands::get_item,
            commands::search_items,
            commands::set_item_fields,
            commands::open_path,
            commands::reveal_path,
            commands::create_category,
            commands::rename_category,
            commands::move_category,
            commands::delete_category,
            commands::set_item_categories,
            commands::list_tags,
            commands::create_tag,
            commands::rename_tag,
            commands::add_tag_alias,
            commands::merge_tags,
            commands::set_item_tags,
            commands::remove_item_tags,
            commands::list_collections,
            commands::save_collection,
            commands::preview_collection,
            commands::list_rules,
            commands::save_rule,
            commands::delete_rule,
            commands::start_rule_job,
            commands::start_autocat_job,
            commands::add_item_categories,
            commands::ensure_thumb,
            commands::start_dupe_job,
            commands::list_dupes,
            commands::start_ocr_job,
            commands::ocr_pending_count,
            commands::backup_now,
            commands::export_port,
            commands::import_port,
            commands::setup_admin,
            commands::session_user,
            commands::login,
            commands::logout,
            commands::change_password,
            commands::list_users,
            commands::create_user,
            commands::delete_user,
            commands::set_user_role,
            commands::list_audit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running vineCurator");
}
