use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use vine_curator_lib::db;
use vine_curator_lib::{categories, port, tags};

fn setup() -> Connection {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "vinecurator_port_test_{}_{}",
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
fn roundtrip_export_import_into_fresh_db() {
    let mut src = setup();
    let shu = categories::create_category(&mut src, None, "书").expect("书");
    categories::create_category(&mut src, Some(shu), "科幻").expect("科幻");
    categories::create_category(&mut src, None, "电影").expect("电影");
    let qf = tags::create_tag(&src, "奇幻").expect("tag");
    tags::add_alias(&src, "fantasy", qf).expect("alias");
    tags::create_tag(&src, "未读").expect("tag2");

    let doc = port::export_doc(&src).expect("export");
    assert_eq!(doc.categories.len(), 3);
    assert_eq!(doc.tags.len(), 2);
    assert_eq!(doc.version, port::PORT_VERSION);

    // 导入全新库
    let mut dst = setup();
    let rep = port::import_doc(&mut dst, &doc).expect("import");
    assert_eq!(rep.categories_created, 3);
    assert_eq!(rep.tags_created, 2);
    assert_eq!(rep.aliases_added, 1);
    assert!(rep.skipped.is_empty(), "skipped={:?}", rep.skipped);

    // 导出一致（忽略 exported_at）
    let mut back = port::export_doc(&dst).expect("export2");
    let mut doc2 = doc.clone();
    back.exported_at = String::new();
    doc2.exported_at = String::new();
    assert_eq!(
        serde_json::to_value(&back).unwrap(),
        serde_json::to_value(&doc2).unwrap(),
        "往返一致"
    );

    // 幂等：同文档再导入零新增
    let rep = port::import_doc(&mut dst, &doc).expect("reimport");
    assert_eq!(rep.categories_created, 0);
    assert_eq!(rep.categories_updated, 0);
    assert_eq!(rep.tags_created, 0);
    assert_eq!(rep.aliases_added, 0);
    assert!(rep.skipped.is_empty());
}

#[test]
fn import_creates_missing_ancestors_and_skips_invalid() {
    let mut conn = setup();
    let json = r#"{
        "version": 1,
        "exported_at": "0",
        "categories": [
            {"path": "a/b/c", "name": "c", "sort_order": 0},
            {"path": "a//b", "name": "b", "sort_order": 0},
            {"path": "", "name": "", "sort_order": 0}
        ],
        "tags": [
            {"name": "正常", "aliases": ["ok-alias"]},
            {"name": "bad,name", "aliases": []},
            {"name": "", "aliases": [""]},
            {"name": "x", "aliases": ["y"]},
            {"name": "z", "aliases": ["y"]}
        ]
    }"#;
    let rep = port::import_json(&mut conn, json).expect("import");
    // 缺失祖先 a、a/b、a/b/c 全补建
    assert_eq!(rep.categories_created, 3, "rep={rep:?}");
    // 非法路径跳过 2 个
    assert_eq!(
        rep.skipped.iter().filter(|s| s.starts_with("分类")).count(),
        2,
        "skipped={:?}",
        rep.skipped
    );
    assert_eq!(rep.tags_created, 3, "正常 + bad,name被跳过 + x");
    assert_eq!(rep.aliases_added, 2, "ok-alias + y");
    // 空标签名跳过；y 被 x 占用后 z 再来被跳过
    assert!(
        rep.skipped.iter().any(|s| s.contains("bad,name")),
        "skipped={:?}",
        rep.skipped
    );
    assert!(
        rep.skipped.iter().any(|s| s.contains("已被其他标签占用")),
        "skipped={:?}",
        rep.skipped
    );

    // 祖先关系正确：a/b 的 parent = a
    let parent: String = conn
        .query_row(
            "SELECT COALESCE((SELECT path FROM categories WHERE id = c.parent_id), '')
             FROM categories c WHERE c.path = 'a/b'",
            [],
            |r| r.get(0),
        )
        .expect("parent");
    assert_eq!(parent, "a");

    // 同文档更新 sort_order → updated 计数
    let json2 = json.replace("\"sort_order\": 0},", "\"sort_order\": 5},");
    let rep2 = port::import_json(&mut conn, &json2).expect("import2");
    assert_eq!(rep2.categories_created, 0);
    assert!(rep2.categories_updated >= 1, "rep={rep2:?}");

    // 恶意内容参数绑定不注入：名字原样存/被跳过，无表结构破坏
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 3);
}

#[test]
fn json_parse_and_version_errors() {
    let mut conn = setup();
    assert!(port::import_json(&mut conn, "not json at all").is_err());

    let v99 = r#"{"version": 99, "exported_at": "0", "categories": [], "tags": []}"#;
    let err = port::import_json(&mut conn, v99).expect_err("版本过高应报错");
    assert!(err.contains("99"), "err={err}");

    // 版本过低/相等可导入（空文档合法）
    let v1 = r#"{"version": 1, "exported_at": "0", "categories": [], "tags": []}"#;
    let rep = port::import_json(&mut conn, v1).expect("空导入");
    assert_eq!(rep.categories_created, 0);
}
