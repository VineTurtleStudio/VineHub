use rusqlite::Connection;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use vine_curator_lib::audit;
use vine_curator_lib::db;

fn setup() -> Connection {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_audit_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("打开");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    conn
}

#[test]
fn record_and_recent_order_filter_limit() {
    let conn = setup();
    audit::record(
        &conn,
        None,
        "create",
        "tag",
        Some("1"),
        r#"{"name":"科幻"}"#,
    )
    .unwrap();
    audit::record(&conn, None, "update", "category", Some("2"), "{}").unwrap();
    audit::record(&conn, None, "delete", "tag", Some("1"), "{}").unwrap();

    let all = audit::recent(&conn, 50, None).unwrap();
    assert_eq!(all.len(), 3);
    // 倒序（最近的在前）
    assert_eq!(all[0].action, "delete");
    assert_eq!(all[2].action, "create");
    // detail 原样保留
    assert_eq!(all[2].detail, r#"{"name":"科幻"}"#);
    // 实体过滤
    let tags = audit::recent(&conn, 50, Some("tag")).unwrap();
    assert_eq!(tags.len(), 2);
    // limit 钳制（0 → 1）
    let top = audit::recent(&conn, 0, None).unwrap();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].action, "delete");
}

#[test]
fn action_whitelist_and_entity_guard() {
    let conn = setup();
    // 非白名单动作拒绝
    assert!(audit::record(&conn, None, "drop_table", "tag", None, "{}").is_err());
    assert!(audit::record(&conn, None, "", "tag", None, "{}").is_err());
    // 空实体拒绝
    assert!(audit::record(&conn, None, "create", "  ", None, "{}").is_err());
    // 白名单全部可用
    for action in audit::ACTIONS {
        audit::record(&conn, None, action, "test", None, "{}")
            .unwrap_or_else(|e| panic!("{action} 应合法: {e}"));
    }
    assert_eq!(
        audit::recent(&conn, 500, None).unwrap().len(),
        audit::ACTIONS.len()
    );
}

#[test]
fn audit_join_shows_username() {
    let conn = setup();
    let id = conn
        .execute(
            "INSERT INTO users (username, password_hash, role) VALUES ('u1', 'x', 'viewer')",
            [],
        )
        .map_err(|e| e.to_string())
        .unwrap();
    let _ = id;
    audit::record(&conn, Some(1), "login", "session", Some("1"), "{}").unwrap();
    let rows = audit::recent(&conn, 10, None).unwrap();
    assert_eq!(rows[0].username.as_deref(), Some("u1"));
    assert_eq!(rows[0].user_id, Some(1));
}

// ---------------------------------------------------------------- 写命令覆盖率守护

/// 必须写审计的命令（阶段三验收：写操作 100% 覆盖）
const MUTATING: &[&str] = &[
    "save_settings",
    "add_root",
    "remove_root",
    "redirect_root",
    "start_scan",
    "start_hash_job",
    "start_meta_job",
    "start_rule_job",
    "start_autocat_job",
    "add_item_categories",
    "cancel_job",
    "set_item_fields",
    "create_category",
    "rename_category",
    "move_category",
    "delete_category",
    "set_item_categories",
    "create_tag",
    "rename_tag",
    "add_tag_alias",
    "merge_tags",
    "set_item_tags",
    "remove_item_tags",
    "save_collection",
    "save_rule",
    "delete_rule",
    "ensure_thumb",
    "start_dupe_job",
    "start_ocr_job",
    "backup_now",
    "export_port",
    "import_port",
    "setup_admin",
    "login",
    "logout",
    "change_password",
    "create_user",
    "delete_user",
    "set_user_role",
];

/// 只读命令（明确豁免审计）
const READONLY: &[&str] = &[
    "probe_tools",
    "get_db_info",
    "get_settings",
    "list_roots",
    "list_items",
    "list_categories",
    "get_overview",
    "get_item",
    "search_items",
    "open_path",
    "reveal_path",
    "list_tags",
    "list_collections",
    "preview_collection",
    "list_rules",
    "list_dupes",
    "ocr_pending_count",
    "session_user",
    "list_users",
    "list_audit",
];

fn command_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0;
    while let Some(p) = src[idx..].find("#[tauri::command") {
        let start = idx + p;
        let rest = &src[start..];
        if let Some(fp) = rest.find("pub fn ") {
            let after = &rest[fp + "pub fn ".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphabetic() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
        idx = start + "#[tauri::command".len();
    }
    out
}

fn body_of<'a>(src: &'a str, name: &str) -> &'a str {
    let pat = format!("pub fn {name}(");
    let s = src
        .find(&pat)
        .unwrap_or_else(|| panic!("未找到命令: {name}"));
    let rest = &src[s..];
    match rest[pat.len()..].find("#[tauri::command") {
        Some(next) => &rest[..pat.len() + next],
        None => rest,
    }
}

#[test]
fn every_command_is_classified_and_write_commands_audit() {
    let src = include_str!("../src/commands.rs");
    let names = command_names(src);
    assert!(
        names.len() >= 45,
        "解析到的命令偏少（{}）：解析逻辑或文件结构可能变了",
        names.len()
    );

    // 1) 每个命令必须归入 写（MUTATING）或 只读（READONLY）之一 —— 新命令必须显式分类
    for name in &names {
        assert!(
            MUTATING.contains(&name.as_str()) || READONLY.contains(&name.as_str()),
            "命令 {name} 未分类：写命令须加入 MUTATING 并插桩审计，只读须加入 READONLY"
        );
    }
    // 反向：清单里的命令都真实存在
    for name in MUTATING.iter().chain(READONLY.iter()) {
        assert!(
            names.contains(&name.to_string()),
            "清单中的命令 {name} 不存在"
        );
    }

    // 2) 写命令函数体必须含审计调用（audit_now / audit::record / audit_job）
    for name in MUTATING {
        let body = body_of(src, name);
        assert!(
            body.contains("audit_now(")
                || body.contains("audit::record(")
                || body.contains("audit_job("),
            "写命令 {name} 缺少审计调用"
        );
    }

    // 3) 只读命令不得写审计（避免噪音）
    for name in READONLY {
        let body = body_of(src, name);
        assert!(
            !body.contains("audit_now(") && !body.contains("audit_job("),
            "只读命令 {name} 不应写审计"
        );
    }
}

#[test]
fn all_commands_registered_in_handler() {
    let src = include_str!("../src/commands.rs");
    let lib = include_str!("../src/lib.rs");
    let names = command_names(src);
    // 提取 generate_handler![ ... ] 块
    let gh = lib
        .find("generate_handler![")
        .expect("lib.rs 应有 generate_handler");
    let block = &lib[gh..];
    let block_end = block.find("])").map(|i| i + 2).unwrap_or(block.len());
    let block = &block[..block_end];
    for name in &names {
        let pat = format!("commands::{name}");
        assert!(block.contains(&pat), "命令 {name} 未注册到 invoke_handler");
    }
}
