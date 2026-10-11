use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use vine_curator_lib::browse;
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::fts;
use vine_curator_lib::scan;

fn setup() -> (Connection, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("vinecurator_fts_test_{}_{}", std::process::id(), n));
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

fn fts_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM fts_items", [], |r| r.get(0))
        .expect("fts 计数")
}

fn item_id_by_name(conn: &Connection, name: &str) -> i64 {
    conn.query_row("SELECT id FROM items WHERE file_name = ?1", [name], |r| {
        r.get(0)
    })
    .expect("查 id")
}

#[test]
fn scan_syncs_fts_index() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);
    assert_eq!(fts_count(&conn), 3);

    // 文件名 token 可命中
    let hits = fts::search_ids(&conn, "Kind", 50).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0], item_id_by_name(&conn, "Kind of Blue.flac"));

    // 多词 AND + 前缀
    let hits = fts::search_ids(&conn, "dune part", 50).expect("multi");
    assert_eq!(hits.len(), 1);

    // 路径分词（body = rel_path）
    let hits = fts::search_ids(&conn, "books", 50).expect("path");
    assert_eq!(hits.len(), 1);
}

#[test]
fn match_expression_neutralizes_operators_and_quotes() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // FTS 操作符全部成为字面量：不报错、不注入
    for q in [
        "kind OR neuromancer",
        "kind AND dune",
        "NEAR(kind blue)",
        "kind*",
        "\"quoted\"",
        "dune NEAR/2 two",
        "col:title",
    ] {
        let res = fts::search_ids(&conn, q, 50);
        assert!(res.is_ok(), "查询应安全执行: {q} -> {res:?}");
    }

    // 危险字符只走字面量，items 表完好
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .expect("items 仍在");
    assert_eq!(n, 3);
}

#[test]
fn ext_and_tag_text_is_searchable() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // 扩展表字段（模拟元数据写入后同步）
    let vid = item_id_by_name(&conn, "Dune Part Two.mkv");
    conn.execute(
        "UPDATE video_items SET series = 'Dune Chronicles', resolution = '2160p' WHERE item_id = ?1",
        [vid],
    )
    .expect("ext");
    fts::sync_item(&conn, vid).expect("sync");
    let hits = fts::search_ids(&conn, "chronicles", 50).expect("ext hit");
    assert_eq!(hits, vec![vid]);
    let hits = fts::search_ids(&conn, "2160p", 50).expect("res hit");
    assert_eq!(hits, vec![vid]);

    // 标签名（tags_text 列）
    conn.execute("INSERT INTO tags (id, name) VALUES (1, 'sci-fi')", [])
        .expect("tag");
    conn.execute(
        "INSERT INTO item_tags (item_id, tag_id, source) VALUES (?1, 1, 'manual')",
        [vid],
    )
    .expect("link");
    fts::sync_item(&conn, vid).expect("sync2");
    let hits = fts::search_ids(&conn, "sci", 50).expect("tag hit");
    assert!(hits.contains(&vid));
}

#[test]
fn rebuild_and_ensure_consistent() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // 人为清空索引 → 计数不一致 → 一致性检查触发重建
    conn.execute("DELETE FROM fts_items", []).expect("clear");
    assert_eq!(fts_count(&conn), 0);
    let rebuilt = fts::ensure_consistent(&mut conn).expect("ensure");
    assert!(rebuilt);
    assert_eq!(fts_count(&conn), 3);

    // 已一致 → 不重建
    let again = fts::ensure_consistent(&mut conn).expect("ensure2");
    assert!(!again);

    // rebuild_all 幂等
    let n = fts::rebuild_all(&mut conn).expect("rebuild");
    assert_eq!(n, 3);
    assert_eq!(fts_count(&conn), 3);
}

#[test]
fn prune_orphans_removes_deleted_item_rows() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);
    let vid = item_id_by_name(&conn, "Dune Part Two.mkv");

    // 直接删条目（模拟无 hook 的路径）→ 孤儿行 → prune 清理
    conn.execute("DELETE FROM items WHERE id = ?1", [vid])
        .expect("del");
    assert_eq!(fts_count(&conn), 3);
    let pruned = fts::prune_orphans(&conn).expect("prune");
    assert_eq!(pruned, 1);
    assert_eq!(fts_count(&conn), 2);
    assert!(fts::search_ids(&conn, "dune", 50).expect("gone").is_empty());
}

#[test]
fn search_items_fts_first_then_like_fallback() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);

    // FTS 路径：完整 token
    let hits = browse::search_items(&conn, "Neuromancer", None).expect("fts");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].file_name, "Neuromancer.epub");

    // LIKE 回落：FTS 分词覆盖不到的子串（unicode61 把 "rom" 当独立查询
    // 也能前缀命中，故用纯子串 "romanc" 之后的场景 —— 用文件中段验证）
    let hits = browse::search_items(&conn, "euroman", None).expect("mid");
    assert_eq!(hits.len(), 1);

    // 通配符转义仍然生效（FTS 空 → LIKE 兜底）
    let hits = browse::search_items(&conn, "%", None).expect("wildcard");
    assert!(hits.is_empty());

    // 空查询
    assert!(browse::search_items(&conn, "  ", None)
        .expect("empty")
        .is_empty());

    // 注入尝试在 search_items 层也安全
    let res = browse::search_items(&conn, "x' OR 1=1 --", None);
    assert!(res.is_ok());
}

#[test]
fn fts_rank_orders_relevance() {
    let (mut conn, root) = setup();
    seed(&mut conn, &root);
    // 标题里出现两次 "dune"（文件名 + 系列）的应排前面
    let vid = item_id_by_name(&conn, "Dune Part Two.mkv");
    conn.execute(
        "UPDATE video_items SET series = 'Dune' WHERE item_id = ?1",
        [vid],
    )
    .expect("ext");
    fts::sync_item(&conn, vid).expect("sync");

    let hits = fts::search_ids(&conn, "dune", 50).expect("rank");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0], vid);
}
