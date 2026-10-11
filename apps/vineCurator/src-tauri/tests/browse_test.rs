use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use vine_curator_lib::browse::{self, ItemFilter};
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::scan;

fn setup() -> (Connection, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_browse_test_{}_{}",
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
    touch(&root.join("music/aaa.flac"), b"a");
    touch(&root.join("music/bbb.mp3"), b"bb");
    touch(&root.join("video/ccc.mkv"), b"ccc");
    touch(&root.join("books/ddd.epub"), b"dddd");
    touch(&root.join("other/eee.txt"), b"e");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    scan::scan_root(conn, root_id, root, &cancel, &mut |_| {}).expect("扫描");
    root_id
}

#[test]
fn list_items_filters_sorts_and_pages() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    // 全量 + 排序
    let page = browse::list_items(&conn, &ItemFilter::default()).expect("list");
    assert_eq!(page.total, 5);
    assert_eq!(page.items.len(), 5);
    let names: Vec<&str> = page.items.iter().map(|i| i.file_name.as_str()).collect();
    assert_eq!(
        names,
        vec!["aaa.flac", "bbb.mp3", "ccc.mkv", "ddd.epub", "eee.txt"]
    );

    // 倒序
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            sort: Some("file_name".into()),
            desc: Some(true),
            ..Default::default()
        },
    )
    .expect("desc");
    assert_eq!(page.items[0].file_name, "eee.txt");

    // 类型过滤
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            type_name: Some("music".into()),
            ..Default::default()
        },
    )
    .expect("type");
    assert_eq!(page.total, 2);
    assert!(page.items.iter().all(|i| i.type_name == "music"));

    // 分页
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            limit: Some(2),
            offset: Some(2),
            sort: Some("file_name".into()),
            ..Default::default()
        },
    )
    .expect("page");
    assert_eq!(page.total, 5);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].file_name, "ccc.mkv");

    // 根目录过滤
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            root_id: Some(root_id),
            ..Default::default()
        },
    )
    .expect("root");
    assert_eq!(page.total, 5);
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            root_id: Some(root_id + 999),
            ..Default::default()
        },
    )
    .expect("wrong root");
    assert_eq!(page.total, 0);
}

#[test]
fn sort_whitelist_blocks_injection() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // 非白名单字段回落到 file_name，不报错也不拼接
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            sort: Some("id; DROP TABLE items;--".into()),
            ..Default::default()
        },
    )
    .expect("注入尝试应安全回落");
    assert_eq!(page.total, 5);

    // items 表仍在
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .expect("表仍在");
    assert_eq!(n, 5);
}

#[test]
fn sort_by_year_and_rating_columns() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    conn.execute(
        "UPDATE items SET year = 1999, rating = 5 WHERE file_name = 'aaa.flac'",
        [],
    )
    .expect("设 aaa");
    conn.execute(
        "UPDATE items SET year = 2024, rating = 1 WHERE file_name = 'ccc.mkv'",
        [],
    )
    .expect("设 ccc");

    // year DESC → 2024 在前（其余 NULL 在尾）
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            sort: Some("year".into()),
            desc: Some(true),
            ..Default::default()
        },
    )
    .expect("year 排序");
    assert_eq!(page.items[0].file_name, "ccc.mkv");

    // rating DESC → 5 分在前
    let page = browse::list_items(
        &conn,
        &ItemFilter {
            sort: Some("rating".into()),
            desc: Some(true),
            ..Default::default()
        },
    )
    .expect("rating 排序");
    assert_eq!(page.items[0].file_name, "aaa.flac");
}

#[test]
fn overview_counts_by_type_and_status() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    let ov = browse::overview(&conn, None).expect("overview");
    assert_eq!(ov.total, 5);
    assert_eq!(ov.missing, 0);
    let music = ov.by_type.iter().find(|c| c.key == "music").unwrap();
    assert_eq!(music.count, 2);

    let ov2 = browse::overview(&conn, Some(root_id)).expect("scoped");
    assert_eq!(ov2.total, 5);
    let ov3 = browse::overview(&conn, Some(root_id + 999)).expect("empty scope");
    assert_eq!(ov3.total, 0);
}

#[test]
fn category_tree_builds_nested_with_counts() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    conn.execute(
        "INSERT INTO categories (id, parent_id, name, path) VALUES (1, NULL, '音乐', '音乐')",
        [],
    )
    .expect("c1");
    conn.execute(
        "INSERT INTO categories (id, parent_id, name, path) VALUES (2, 1, '爵士', '音乐/爵士')",
        [],
    )
    .expect("c2");
    conn.execute(
        "INSERT INTO categories (id, parent_id, name, path) VALUES (3, NULL, '电影', '电影')",
        [],
    )
    .expect("c3");

    // 把两个 music 条目挂到 约/爵士
    let item_ids: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT id FROM items WHERE type = 'music'")
            .expect("prep");
        let rows = stmt
            .query_map([], |r| r.get(0))
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        rows
    };
    for id in item_ids {
        conn.execute(
            "INSERT INTO item_categories (item_id, category_id) VALUES (?1, 2)",
            [id],
        )
        .expect("link");
    }
    let _ = root_id;

    let tree = browse::list_categories(&conn).expect("tree");
    assert_eq!(tree.len(), 2);
    let music = tree.iter().find(|n| n.name == "音乐").unwrap();
    assert_eq!(music.item_count, 0); // 只计直接挂载
    assert_eq!(music.children.len(), 1);
    let jazz = &music.children[0];
    assert_eq!(jazz.name, "爵士");
    assert_eq!(jazz.item_count, 2);
    let film = tree.iter().find(|n| n.name == "电影").unwrap();
    assert_eq!(film.children.len(), 0);
}

#[test]
fn get_item_detail_includes_ext_tags_metadata() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    // 模拟元数据 + 标签
    conn.execute(
        "UPDATE items SET metadata = '{\"probe\":true}', year = 2020 WHERE type = 'video'",
        [],
    )
    .expect("meta");
    conn.execute("INSERT INTO tags (id, name) VALUES (1, '4K')", [])
        .expect("tag");
    let video_id: i64 = conn
        .query_row("SELECT id FROM items WHERE type = 'video'", [], |r| {
            r.get(0)
        })
        .expect("vid");
    conn.execute(
        "INSERT INTO item_tags (item_id, tag_id, source) VALUES (?1, 1, 'manual')",
        [video_id],
    )
    .expect("item_tag");
    conn.execute(
        "UPDATE video_items SET resolution = '1920x1080', codec = 'h264' WHERE item_id = ?1",
        [video_id],
    )
    .expect("ext");

    let d = browse::get_item(&conn, video_id).expect("detail");
    assert_eq!(d.item.type_name, "video");
    assert_eq!(d.item.year, Some(2020));
    assert_eq!(d.metadata, "{\"probe\":true}");
    assert!(d.root_path.ends_with("root"));
    let res = d.ext.iter().find(|f| f.name == "resolution").unwrap();
    assert_eq!(res.value.as_deref(), Some("1920x1080"));
    let codec = d.ext.iter().find(|f| f.name == "codec").unwrap();
    assert_eq!(codec.value.as_deref(), Some("h264"));
    assert_eq!(d.tags.len(), 1);
    assert_eq!(d.tags[0].name, "4K");
    assert_eq!(d.tags[0].source, "manual");

    // 不存在的条目
    assert!(browse::get_item(&conn, 999999).is_err());
    let _ = root_id;
}

#[test]
fn search_matches_name_and_path_escapes_wildcards() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    let hits = browse::search_items(&conn, "bbb", None).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].file_name, "bbb.mp3");

    // 路径也匹配
    let hits = browse::search_items(&conn, "music/", None).expect("path search");
    assert_eq!(hits.len(), 2);

    // 通配符转义：`%` 不应匹配全部
    let hits = browse::search_items(&conn, "%", None).expect("wildcard");
    assert!(hits.is_empty());

    // 空查询 → 空结果
    assert!(browse::search_items(&conn, "   ", None)
        .expect("empty")
        .is_empty());
}
