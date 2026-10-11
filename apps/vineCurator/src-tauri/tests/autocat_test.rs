use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vine_curator_lib::autocat;
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::scan;

fn setup() -> (Connection, PathBuf) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_autocat_test_{}_{}",
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
    touch(&root.join("Rock/Queen/Album/01.flac"), b"a");
    touch(&root.join("Rock/Jazz/Kind of Blue/02.flac"), b"bb");
    touch(&root.join("single.mp3"), b"ccc");
    let cancel = AtomicBool::new(false);
    scan::scan_root(conn, root_id, root, &cancel, &mut |_| {}).expect("扫描");
    root_id
}

fn run(
    conn: &mut Connection,
    root_id: Option<i64>,
    depth: usize,
    cancel: &AtomicBool,
) -> autocat::AutoCatStats {
    let mut events = 0usize;
    let mut emit = |p: &autocat::AutoCatProgress| {
        if p.phase == "finished" {
            events += 1;
        }
    };
    let out = autocat::run(conn, root_id, depth, cancel, &mut emit).expect("跑自动分类");
    assert_eq!(events, 1, "恰好一次 finished 事件");
    out
}

fn item_cat(conn: &Connection, rel: &str) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT c.path, ic.source FROM item_categories ic
             JOIN categories c ON c.id = ic.category_id
             JOIN items i ON i.id = ic.item_id
             WHERE i.rel_path = ?1 ORDER BY c.path",
        )
        .expect("查询");
    let rows = stmt
        .query_map([rel], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("映射");
    let mut v = Vec::new();
    for row in rows {
        v.push(row.expect("行"));
    }
    v
}

#[test]
fn mirrors_parent_folders_up_to_depth() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    let stats = run(&mut conn, Some(root_id), 2, &AtomicBool::new(false));
    assert_eq!(stats.items, 3);
    assert_eq!(stats.linked, 2, "根下文件不归类");
    assert_eq!(stats.unlinked, 0);

    assert_eq!(
        item_cat(&conn, "Rock/Queen/Album/01.flac"),
        vec![("Rock/Queen".to_string(), "auto".to_string())]
    );
    assert_eq!(
        item_cat(&conn, "Rock/Jazz/Kind of Blue/02.flac"),
        vec![("Rock/Jazz".to_string(), "auto".to_string())]
    );
    assert!(item_cat(&conn, "single.mp3").is_empty());

    // 树结构存在
    let paths: i64 = conn
        .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(paths, 3, "Rock + Rock/Queen + Rock/Jazz");
}

#[test]
fn rerun_replaces_stale_auto_and_preserves_manual() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    run(&mut conn, Some(root_id), 1, &AtomicBool::new(false));
    assert_eq!(
        item_cat(&conn, "Rock/Queen/Album/01.flac"),
        vec![("Rock".to_string(), "auto".to_string())]
    );

    // 手动加一个收藏分类（source 默认 manual）
    let fav = vine_curator_lib::categories::create_category(&mut conn, None, "收藏").expect("收藏");
    let item_id: i64 = conn
        .query_row(
            "SELECT id FROM items WHERE rel_path = 'Rock/Queen/Album/01.flac'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO item_categories (item_id, category_id) VALUES (?1, ?2)",
        rusqlite::params![item_id, fav],
    )
    .unwrap();

    // 深度改 2 → 过期 auto(Rock) 移除，auto 换成 Rock/Queen，manual 收藏保留
    let stats = run(&mut conn, Some(root_id), 2, &AtomicBool::new(false));
    assert_eq!(stats.unlinked, 2, "两条 auto(Rock) 都被换掉");
    assert_eq!(stats.linked, 2);
    assert_eq!(
        item_cat(&conn, "Rock/Queen/Album/01.flac"),
        vec![
            ("Rock/Queen".to_string(), "auto".to_string()),
            ("收藏".to_string(), "manual".to_string())
        ]
    );
}

#[test]
fn cancel_and_depth_zero_write_nothing() {
    let (mut conn, root) = setup();
    let root_id = seed(&mut conn, &root);

    let cancel = AtomicBool::new(true);
    let stats = run(&mut conn, Some(root_id), 2, &cancel);
    assert_eq!(stats.items, 0);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM item_categories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "取消零写入");

    let stats = run(&mut conn, Some(root_id), 0, &AtomicBool::new(false));
    assert_eq!(stats.items, 0);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM item_categories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "depth=0 零写入");
}

#[test]
fn desired_path_edge_cases() {
    assert_eq!(autocat::desired_path("a/b/c.mp3", 2), Some("a/b".into()));
    assert_eq!(autocat::desired_path("a/b/c.mp3", 5), Some("a/b".into()));
    assert_eq!(autocat::desired_path("top.mp3", 3), None);
    assert_eq!(autocat::desired_path("a/b/c.mp3", 0), None);
    assert_eq!(autocat::desired_path("./a//b/c.mp3", 2), Some("a/b".into()));
}
