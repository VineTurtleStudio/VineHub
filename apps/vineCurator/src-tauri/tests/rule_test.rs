use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::{fts, rules, scan};

fn setup() -> (Connection, PathBuf) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_rule_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("开库");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("根目录");
    (conn, root)
}

fn touch(path: &std::path::Path, contents: &[u8]) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).expect("父目录");
    }
    fs::write(path, contents).expect("写文件");
}

fn seed(conn: &mut Connection, root: &std::path::Path) -> i64 {
    let root_id = repo::add_root(conn, &root.to_string_lossy(), "test")
        .expect("加根目录")
        .id;
    touch(&root.join("music/Kind of Blue.flac"), b"a");
    touch(&root.join("video/Dune Part Two.mkv"), b"bb");
    touch(&root.join("books/Neuromancer.epub"), b"ccc");
    let cancel = AtomicBool::new(false);
    scan::scan_root(conn, root_id, root, &cancel, &mut |_| {}).expect("扫描");
    root_id
}

fn tag_source(conn: &Connection, item_id: i64, name: &str) -> Option<String> {
    conn.query_row(
        "SELECT it.source FROM item_tags it JOIN tags t ON t.id = it.tag_id
         WHERE it.item_id = ?1 AND t.name = ?2",
        rusqlite::params![item_id, name],
        |r| r.get(0),
    )
    .ok()
}

fn rule_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM rules", [], |r| r.get(0))
        .expect("规则计数")
}

#[test]
fn save_rule_enforces_whitelists() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    // 字段注入 → 拒绝
    let e = rules::save_rule(
        &conn,
        None,
        "inj",
        true,
        0,
        r#"{"field":"id; DROP TABLE items;--","op":"eq","value":1}"#,
        r#"[{"op":"add_tag","value":"x"}]"#,
    )
    .expect_err("字段注入应被拒绝");
    assert!(e.contains("白名单") || e.contains("字段"), "实际: {e}");

    // 动作 op 白名单 → 拒绝
    let e = rules::save_rule(
        &conn,
        None,
        "bad-op",
        true,
        0,
        r#"{"field":"type","op":"eq","value":"book"}"#,
        r#"[{"op":"drop_table","value":"items"}]"#,
    )
    .expect_err("未知 op 应被拒绝");
    assert!(e.contains("不支持的动作"), "实际: {e}");

    // 标签名含逗号 → 拒绝
    let e = rules::save_rule(
        &conn,
        None,
        "bad-tag",
        true,
        0,
        r#"{"field":"type","op":"eq","value":"book"}"#,
        r#"[{"op":"add_tag","value":"a,b"}]"#,
    )
    .expect_err("逗号标签应被拒绝");
    assert!(e.contains("逗号"), "实际: {e}");

    // 合法保存 → 同名冲突
    let id = rules::save_rule(
        &conn,
        None,
        "未读打标",
        true,
        0,
        r#"{"all":[{"field":"type","op":"eq","value":"book"}]}"#,
        r#"[{"op":"add_tag","value":"未读"}]"#,
    )
    .expect("保存");
    let e = rules::save_rule(
        &conn,
        None,
        "未读打标",
        true,
        0,
        r#"{"field":"type","op":"eq","value":"book"}"#,
        r#"[{"op":"add_tag","value":"未读"}]"#,
    )
    .expect_err("同名应被拒绝");
    assert!(e.contains("同名"), "实际: {e}");

    // 覆盖默认 '[]'：空数组 = 全量条件
    rules::save_rule(
        &conn,
        Some(id),
        "未读打标",
        false,
        3,
        "[]",
        r#"[{"op":"add_tag","value":"未读"}]"#,
    )
    .expect("更新");
    let rows = rules::list_rules(&conn).expect("list");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].enabled);
    assert_eq!(rows[0].priority, 3);

    assert_eq!(rule_count(&conn), 1);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .expect("items 完好");
    assert_eq!(n, 3, "注入不能破坏 items 表");
    let _ = root_id;
}

#[test]
fn run_rules_tags_matches_with_rule_source_and_fts() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);
    rules::save_rule(
        &conn,
        None,
        "book 未读",
        true,
        0,
        r#"{"all":[{"field":"type","op":"eq","value":"book"}]}"#,
        r#"[{"op":"add_tag","value":"未读"}]"#,
    )
    .expect("保存");

    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let stats = {
        let mut emit = |p: &rules::RuleProgress| events.push(p.clone());
        rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run")
    };
    assert_eq!(stats.rules_run, 1);
    assert_eq!(stats.matched, 1, "只有 book 命中");
    assert_eq!(stats.tagged, 1);

    let book_id: i64 = conn
        .query_row(
            "SELECT id FROM items WHERE file_name = 'Neuromancer.epub'",
            [],
            |r| r.get(0),
        )
        .expect("book id");
    let music_id: i64 = conn
        .query_row(
            "SELECT id FROM items WHERE file_name = 'Kind of Blue.flac'",
            [],
            |r| r.get(0),
        )
        .expect("music id");
    assert_eq!(tag_source(&conn, book_id, "未读").as_deref(), Some("rule"));
    assert_eq!(tag_source(&conn, music_id, "未读"), None);

    // FTS 同步：标签可检索
    let hits = fts::search_ids(&conn, "未读", 50).expect("fts");
    assert!(hits.contains(&book_id), "标签应进 FTS: {hits:?}");

    // 进度事件：running + finished
    assert!(events.iter().any(|e| e.status == "running"));
    let last = events.last().expect("finished 事件");
    assert_eq!(last.phase, "finished");
    assert_eq!(last.status, "done");
    assert_eq!(last.stats.tagged, 1);
}

#[test]
fn run_rules_idempotent_and_keeps_manual_source() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);
    let book_id: i64 = conn
        .query_row(
            "SELECT id FROM items WHERE file_name = 'Neuromancer.epub'",
            [],
            |r| r.get(0),
        )
        .expect("book id");

    // 人工先打同名标签
    let written =
        vine_curator_lib::tags::add_tags_to_items(&mut conn, &[book_id], &["未读".to_string()])
            .expect("手动打标");
    assert_eq!(written.len(), 1);

    rules::save_rule(
        &conn,
        None,
        "book 未读",
        true,
        0,
        r#"{"all":[{"field":"type","op":"eq","value":"book"}]}"#,
        r#"[{"op":"add_tag","value":"未读"}]"#,
    )
    .expect("保存");

    let cancel = AtomicBool::new(false);
    let stats = {
        let mut emit = |_p: &rules::RuleProgress| {};
        rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run 1")
    };
    assert_eq!(stats.tagged, 0, "已有打标不重复写");

    // 再跑一遍仍然幂等
    let stats = {
        let mut emit = |_p: &rules::RuleProgress| {};
        rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run 2")
    };
    assert_eq!(stats.rules_run, 1);
    assert_eq!(stats.tagged, 0);

    // 来源仍是 manual（规则不降级人工打标）
    assert_eq!(
        tag_source(&conn, book_id, "未读").as_deref(),
        Some("manual"),
        "manual 优先，规则不覆盖"
    );
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id
             WHERE it.item_id = ?1 AND t.name = '未读'",
            [book_id],
            |r| r.get(0),
        )
        .expect("计数");
    assert_eq!(n, 1, "只有一行打标");
}

#[test]
fn disabled_rules_skipped_and_root_scope_works() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);
    // 第二个根目录
    let root2 = root.parent().expect("parent").join("root2");
    fs::create_dir_all(root2.join("books")).expect("root2");
    touch(&root2.join("books/Octavia.epub"), b"d");
    let root2_id = repo::add_root(&mut conn, &root2.to_string_lossy(), "test2")
        .expect("根2")
        .id;
    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut conn, root2_id, &root2, &cancel, &mut |_| {}).expect("扫描2");

    // 规则 A：启用；规则 B：停用
    rules::save_rule(
        &conn,
        None,
        "A",
        true,
        10,
        r#"{"field":"type","op":"eq","value":"book"}"#,
        r#"[{"op":"add_tag","value":"书"}]"#,
    )
    .expect("A");
    rules::save_rule(
        &conn,
        None,
        "B",
        false,
        5,
        r#"{"field":"type","op":"eq","value":"book"}"#,
        r#"[{"op":"add_tag","value":"停用"}]"#,
    )
    .expect("B");

    // 只跑 root1 范围
    let stats = {
        let mut emit = |_p: &rules::RuleProgress| {};
        rules::run_rules(&mut conn, Some(root_id), &cancel, &mut emit).expect("run")
    };
    assert_eq!(stats.rules_run, 1, "停用规则不跑");
    assert_eq!(stats.matched, 1, "范围只含 root1 的 book");

    let tagged: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id
             WHERE t.name = '书'",
            [],
            |r| r.get(0),
        )
        .expect("计数");
    assert_eq!(tagged, 1);
    let stopped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id
             WHERE t.name = '停用'",
            [],
            |r| r.get(0),
        )
        .expect("计数");
    assert_eq!(stopped, 0, "停用规则不打标");
}

#[test]
fn cancel_flag_stops_before_any_write() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);
    rules::save_rule(
        &conn,
        None,
        "R",
        true,
        0,
        "[]",
        r#"[{"op":"add_tag","value":"全部"}]"#,
    )
    .expect("保存");

    let cancel = AtomicBool::new(true);
    let mut events = Vec::new();
    let stats = {
        let mut emit = |p: &rules::RuleProgress| events.push(p.clone());
        rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run")
    };
    assert_eq!(stats.rules_run, 0);
    assert_eq!(stats.tagged, 0);
    assert_eq!(events.last().expect("事件").status, "cancelled");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM item_tags", [], |r| r.get(0))
        .expect("计数");
    assert_eq!(n, 0);
}

#[test]
fn no_enabled_rules_emits_finished() {
    let (conn, _root) = setup();
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let stats = {
        let mut conn = conn;
        let mut emit = |p: &rules::RuleProgress| events.push(p.clone());
        rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run")
    };
    assert_eq!(stats.rules_run, 0);
    let last = events.last().expect("事件");
    assert_eq!(last.status, "done");
    assert!(last.message.contains("无启用规则"));
}

#[test]
fn set_category_action_creates_path_and_keeps_manual() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // 超长段名 → 拒绝
    let e = rules::validate_actions(&format!(
        r#"[{{"op":"set_category","value":"a/{}"}}]"#,
        "长".repeat(101)
    ))
    .expect_err("非法段名应被拒绝");
    assert!(e.contains("名称"), "实际: {e}");

    rules::save_rule(
        &conn,
        None,
        "归到4K",
        true,
        0,
        r#"{"all":[{"field":"type","op":"eq","value":"video"}]}"#,
        r#"[{"op":"set_category","value":"观影/4K"}]"#,
    )
    .expect("保存");

    // 手动先给视频条目挂一个分类
    let manual_cat =
        vine_curator_lib::categories::create_category(&mut conn, None, "手动区").expect("手动区");
    let video_id: i64 = conn
        .query_row("SELECT id FROM items WHERE type = 'video'", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO item_categories (item_id, category_id, source) VALUES (?1, ?2, 'manual')",
        rusqlite::params![video_id, manual_cat],
    )
    .unwrap();

    let cancel = AtomicBool::new(false);
    let mut emit = |_: &rules::RuleProgress| {};
    let stats = rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run");
    assert_eq!(stats.categorized, 1);
    assert_eq!(stats.tagged, 0);

    let rows: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT c.path, ic.source FROM item_categories ic
                 JOIN categories c ON c.id = ic.category_id
                 WHERE ic.item_id = ?1 ORDER BY c.path",
            )
            .unwrap();
        let rs = stmt
            .query_map([video_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        rs.map(|r| r.unwrap()).collect()
    };
    assert_eq!(
        rows,
        vec![
            ("手动区".to_string(), "manual".to_string()),
            ("观影/4K".to_string(), "rule".to_string())
        ]
    );

    // 幂等重跑：零新增
    let stats = rules::run_rules(&mut conn, None, &cancel, &mut emit).expect("run");
    assert_eq!(stats.categorized, 0, "已有关联不重复计入");
}
