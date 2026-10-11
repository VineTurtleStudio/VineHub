use rusqlite::Connection;
use std::fs;
use std::sync::atomic::AtomicBool;
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::scan;
use vine_curator_lib::smart;

fn setup() -> (Connection, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_smart_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("开库");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("根目录");
    (conn, root)
}

fn touch(path: &std::path::Path) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).expect("父目录");
    }
    fs::write(path, b"x").expect("写文件");
}

/// 3 个条目：music/flac、video/mkv、book/epub
fn seed(conn: &mut Connection, root: &std::path::Path) {
    let root_id = repo::add_root(conn, &root.to_string_lossy(), "t")
        .expect("root")
        .id;
    touch(&root.join("music/Kind of Blue.flac"));
    touch(&root.join("video/Dune Part Two.mkv"));
    touch(&root.join("books/Neuromancer.epub"));
    let cancel = AtomicBool::new(false);
    scan::scan_root(conn, root_id, root, &cancel, &mut |_| {}).expect("scan");
}

fn preview_names(conn: &Connection, query_json: &str) -> Vec<String> {
    let (_, items) = smart::preview(conn, query_json, None).expect("preview");
    items.into_iter().map(|i| i.file_name).collect()
}

#[test]
fn whitelist_rejects_unknown_fields_in_filters_and_sort() {
    let (conn, _) = setup();
    // 过滤字段不在白名单
    for field in [
        "id",
        "root_id",
        "rel_path",
        "metadata",
        "missing; DROP TABLE items",
    ] {
        let q = format!(r#"{{"filter":{{"field":"{field}","op":"eq","value":"x"}}}}"#);
        assert!(
            smart::preview(&conn, &q, None).is_err(),
            "字段 {field} 应被白名单拒绝"
        );
    }
    // 排序字段不在白名单
    let q = r#"{"sort":[{"field":"metadata","dir":"asc"}]}"#;
    assert!(smart::preview(&conn, q, None).is_err());
    // 集合保存同样先校验
    assert!(smart::save_collection(
        &conn,
        None,
        "坏集合",
        r#"{"filter":{"field":"id","op":"eq","value":1}}"#,
        0
    )
    .is_err());
    // items 表仍在
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .expect("items");
    assert_eq!(n, 0);
}

#[test]
fn values_never_interpolate_into_sql() {
    let (conn, _) = setup();
    // 值里带引号 / 分号 / DROP：绑定后只是字面量，查询不报错、表不被破坏
    let q = r#"{"filter":{"field":"status","op":"eq","value":"inbox'; DROP TABLE items;--"}}"#;
    let (total, items) = smart::preview(&conn, q, None).expect("绑定值查询");
    assert_eq!(total, 0);
    assert!(items.is_empty());
    // 再次编译也不把值拼进 where
    let (where_sql, _, _, _) = smart::compile_query(q).expect("compile");
    assert!(!where_sql.contains("DROP"), "where 不含用户值: {where_sql}");

    // contains 的 % _ 转义后按字面匹配
    let q2 = r#"{"filter":{"field":"status","op":"contains","value":"100%"}}"#;
    assert!(smart::preview(&conn, q2, None).is_ok());
}

#[test]
fn filters_sort_limit_execute_correctly() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // type 等值
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"type","op":"eq","value":"video"}}"#,
    );
    assert_eq!(names, vec!["Dune Part Two.mkv"]);

    // all：type=music AND status=inbox
    let q = r#"{"filter":{"all":[
        {"field":"type","op":"eq","value":"music"},
        {"field":"status","op":"eq","value":"inbox"}]}}"#;
    assert_eq!(preview_names(&conn, q).len(), 1);

    // any：music 或 video
    let q = r#"{"filter":{"any":[
        {"field":"type","op":"eq","value":"music"},
        {"field":"type","op":"eq","value":"video"}]}}"#;
    assert_eq!(preview_names(&conn, q).len(), 2);

    // not
    let names = preview_names(
        &conn,
        r#"{"filter":{"not":{"field":"type","op":"eq","value":"music"}}}"#,
    );
    assert_eq!(names.len(), 2);

    // in + 排序 + limit
    let q = r#"{"filter":{"field":"type","op":"in","value":["music","book"]},
                 "sort":[{"field":"file_name","dir":"desc"}],"limit":1}"#;
    let (total, items) = smart::preview(&conn, q, None).expect("in+sort+limit");
    assert_eq!(total, 2);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].file_name, "Neuromancer.epub");

    // 数值比较（size > 0）
    let q = r#"{"filter":{"field":"size","op":"gt","value":0}}"#;
    assert_eq!(preview_names(&conn, q).len(), 3);
    // 文本字段用数值 op 报错
    let q = r#"{"filter":{"field":"type","op":"gt","value":1}}"#;
    assert!(smart::preview(&conn, q, None).is_err());
    // 排序方向非法
    let q = r#"{"sort":[{"field":"file_name","dir":"sideways"}]}"#;
    assert!(smart::preview(&conn, q, None).is_err());
}

#[test]
fn tag_and_category_relations() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    let item = conn
        .query_row(
            "SELECT id FROM items WHERE file_name = 'Kind of Blue.flac'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .expect("item");
    conn.execute("INSERT INTO tags (name) VALUES ('4K')", [])
        .expect("tag");
    conn.execute(
        "INSERT INTO item_tags (item_id, tag_id) VALUES (?1, 1)",
        [item],
    )
    .expect("link");
    conn.execute(
        "INSERT INTO categories (name, path) VALUES ('音乐', '音乐')",
        [],
    )
    .expect("category");
    conn.execute(
        "INSERT INTO item_categories (item_id, category_id) VALUES (?1, 1)",
        [item],
    )
    .expect("cat link");

    // tag eq / neq
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"tag","op":"eq","value":"4K"}}"#,
    );
    assert_eq!(names, vec!["Kind of Blue.flac"]);
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"tag","op":"neq","value":"4K"}}"#,
    );
    assert_eq!(names.len(), 2);
    // 别名命中同标签
    conn.execute(
        "INSERT INTO tag_aliases (alias, tag_id) VALUES ('四K', 1)",
        [],
    )
    .expect("alias");
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"tag","op":"eq","value":"四K"}}"#,
    );
    assert_eq!(names, vec!["Kind of Blue.flac"]);
    // tag value 必须是字符串
    assert!(smart::preview(
        &conn,
        r#"{"filter":{"field":"tag","op":"eq","value":1}}"#,
        None
    )
    .is_err());

    // category by id / by path
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"category","op":"eq","value":1}}"#,
    );
    assert_eq!(names, vec!["Kind of Blue.flac"]);
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"category","op":"eq","value":"音乐"}}"#,
    );
    assert_eq!(names, vec!["Kind of Blue.flac"]);
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"category","op":"neq","value":1}}"#,
    );
    assert_eq!(names.len(), 2);

    // ocr_status（book_items 扩展表，未探测 → 无行 → eq 无命中）
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"ocr_status","op":"eq","value":"done"}}"#,
    );
    assert!(names.is_empty());
    // 扫描已建占位行 → 更新 ocr_status
    let book_id: i64 = conn
        .query_row(
            "SELECT id FROM items WHERE file_name = 'Neuromancer.epub'",
            [],
            |r| r.get(0),
        )
        .expect("book");
    conn.execute(
        "UPDATE book_items SET ocr_status = 'done' WHERE item_id = ?1",
        [book_id],
    )
    .expect("book row");
    let names = preview_names(
        &conn,
        r#"{"filter":{"field":"ocr_status","op":"eq","value":"done"}}"#,
    );
    assert_eq!(names, vec!["Neuromancer.epub"]);
}

#[test]
fn collection_crud_roundtrip() {
    let (conn, _) = setup();
    let q = r#"{"filter":{"field":"type","op":"eq","value":"video"},"limit":50}"#;

    // 新建
    let id = smart::save_collection(&conn, None, "未看视频", q, 0).expect("create");
    // 同名拒绝
    assert!(smart::save_collection(&conn, None, "未看视频", q, 0).is_err());
    // 空名拒绝
    assert!(smart::save_collection(&conn, None, "  ", q, 0).is_err());
    // 非法 query_json 拒绝
    assert!(smart::save_collection(
        &conn,
        None,
        "坏的",
        r#"{"filter":{"field":"id","op":"eq","value":1}}"#,
        0
    )
    .is_err());
    assert!(smart::save_collection(&conn, None, "坏的", "{not json", 0).is_err());

    // 列表
    let list = smart::list_collections(&conn).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "未看视频");
    assert_eq!(list[0].query_json, q);

    // 更新
    let q2 = r#"{"filter":{"field":"status","op":"neq","value":"done"}}"#;
    let same = smart::save_collection(&conn, Some(id), "未看视频2", q2, 3).expect("update");
    assert_eq!(same, id);
    let list = smart::list_collections(&conn).expect("list2");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "未看视频2");
    assert_eq!(list[0].query_json, q2);
    assert_eq!(list[0].sort_order, 3);

    // 更新不存在的 id
    assert!(smart::save_collection(&conn, Some(9999), "x", q, 0).is_err());
}

#[test]
fn seed_example_collections_is_idempotent() {
    let (conn, _) = setup();
    smart::seed_example_collections(&conn).expect("seed 1");
    smart::seed_example_collections(&conn).expect("seed 2（幂等）");

    let rows = smart::list_collections(&conn).expect("list");
    assert_eq!(rows.len(), 3, "恰好三个示例集合");
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"未读且=科幻"));
    assert!(names.contains(&"4K 未看"));
    assert!(names.contains(&"待 OCR"));

    // 全部 query_json 过白名单编译 + 能执行
    for r in &rows {
        smart::preview(&conn, &r.query_json, None)
            .unwrap_or_else(|e| panic!("集合「{}」不可用: {e}", r.name));
    }

    // 用户删光后不再重新播种
    conn.execute("DELETE FROM smart_collections", [])
        .expect("清空");
    smart::seed_example_collections(&conn).expect("seed 3");
    let rows = smart::list_collections(&conn).expect("list2");
    assert!(rows.is_empty(), "标记存在则不重播");
}
