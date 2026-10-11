use rusqlite::Connection;
use std::fs;
use std::sync::atomic::AtomicBool;
use vine_curator_lib::db;
use vine_curator_lib::fts;
use vine_curator_lib::scan;
use vine_curator_lib::tags;

fn setup() -> Connection {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("vinecurator_tag_test_{}_{}", std::process::id(), n));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("开库");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    conn
}

fn seed_item(conn: &Connection, rel: &str) -> i64 {
    if conn
        .query_row("SELECT COUNT(*) FROM roots", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        == 0
    {
        conn.execute("INSERT INTO roots (path, label) VALUES ('/r', 'test')", [])
            .expect("root");
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name) VALUES (1, ?1, ?2)",
        rusqlite::params![rel, name],
    )
    .expect("insert item");
    conn.last_insert_rowid()
}

fn tag_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
        .expect("tags count")
}

fn link_count(conn: &Connection, item_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM item_tags WHERE item_id = ?1",
        [item_id],
        |r| r.get(0),
    )
    .expect("link count")
}

fn tags_of(conn: &Connection, item_id: i64) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.name FROM item_tags it JOIN tags t ON t.id = it.tag_id
             WHERE it.item_id = ?1 ORDER BY t.name",
        )
        .expect("prepare");
    let rows = stmt
        .query_map([item_id], |r| r.get::<_, String>(0))
        .expect("map");
    rows.filter_map(|r| r.ok()).collect()
}

#[test]
fn resolve_creates_and_reuses_tags() {
    let conn = setup();
    let a = tags::create_tag(&conn, "爵士").expect("create");
    let b = tags::create_tag(&conn, "爵士").expect("idempotent");
    assert_eq!(a, b, "重复创建应幂等返回同 id");
    assert_eq!(tag_count(&conn), 1);

    // trim 归一化
    let c = tags::create_tag(&conn, "  4K  ").expect("trim");
    let d = tags::create_tag(&conn, "4K").expect("same");
    assert_eq!(c, d);
    assert_eq!(tag_count(&conn), 2);

    // 非法名
    assert!(tags::create_tag(&conn, "").is_err());
    assert!(tags::create_tag(&conn, "a,b").is_err());
    assert!(tags::create_tag(&conn, &"x".repeat(61)).is_err());
}

#[test]
fn alias_resolution_and_conflicts() {
    let conn = setup();
    let jazz = tags::create_tag(&conn, "jazz").expect("jazz");
    let _funk = tags::create_tag(&conn, "funk").expect("funk");

    tags::add_alias(&conn, "爵士", jazz).expect("alias");
    // 别名解析到同 id（通过打标验证）
    let mut conn = conn;
    let item = seed_item(&conn, "a.flac");
    let written =
        tags::add_tags_to_items(&mut conn, &[item], &["爵士".to_string()]).expect("tag via alias");
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].tag_id, jazz);
    assert_eq!(tags_of(&conn, item), vec!["jazz".to_string()]);

    // 指向别的标签名的别名被拒
    assert!(tags::add_alias(&conn, "funk2", jazz).is_ok());
    assert!(
        tags::add_alias(&conn, "funk", jazz).is_err(),
        "funk 已是标签名"
    );
}

#[test]
fn rename_tag_semantics() {
    let conn = setup();
    let id = tags::create_tag(&conn, "rock").expect("rock");
    tags::rename_tag(&conn, id, "摇滚").expect("rename");
    let name: String = conn
        .query_row("SELECT name FROM tags WHERE id = ?1", [id], |r| r.get(0))
        .expect("name");
    assert_eq!(name, "摇滚");

    // 占用别的标签名报错
    let other = tags::create_tag(&conn, "pop").expect("pop");
    assert!(tags::rename_tag(&conn, other, "摇滚").is_err());
    // 改回自己名无害
    assert!(tags::rename_tag(&conn, other, "pop").is_ok());
    // 不存在的标签
    assert!(tags::rename_tag(&conn, 9999, "x").is_err());
}

#[test]
fn merge_dedupes_links_and_moves_aliases() {
    let mut conn = setup();
    let jazz = tags::create_tag(&conn, "jazz").expect("jazz");
    let jazz2 = tags::create_tag(&conn, "jazz2").expect("jazz2");
    tags::add_alias(&conn, "爵士", jazz2).expect("alias on src");

    let item1 = seed_item(&conn, "a.flac");
    let item2 = seed_item(&conn, "b.flac");
    // item1 同时挂两标签（合并时须去重），item2 只挂 src
    tags::add_tags_to_items(&mut conn, &[item1], &["jazz".to_string()]).expect("tag1");
    tags::add_tags_to_items(&mut conn, &[item1], &["jazz2".to_string()]).expect("tag2");
    tags::add_tags_to_items(&mut conn, &[item2], &["jazz2".to_string()]).expect("tag3");
    assert_eq!(link_count(&conn, item1), 2);

    // jazz2 并入 jazz：item1 两标签都有 → 冲突去重 0 行；item2 仅 src → 迁 1 行
    let moved = tags::merge_tags(&mut conn, jazz2, jazz).expect("merge");
    assert_eq!(moved, 1, "仅 item2 的 1 条迁移");
    assert_eq!(tag_count(&conn), 1);
    // item1 去重后只有一条
    assert_eq!(tags_of(&conn, item1), vec!["jazz".to_string()]);
    assert_eq!(tags_of(&conn, item2), vec!["jazz".to_string()]);

    // 别名随迁：重复打标幂等
    let written = tags::add_tags_to_items(&mut conn, &[item2], &["jazz".to_string()])
        .expect("no-op (already tagged)");
    assert!(written.is_empty(), "重复打标不产生变更");
    // 「爵士」仍解析到 jazz（别名已迁走，不新建）
    let _ = tags::create_tag(&conn, "爵士");
    assert_eq!(tag_count(&conn), 1);

    // 合并到自身 / 不存在的标签
    assert!(tags::merge_tags(&mut conn, jazz, jazz).is_err());
    assert!(tags::merge_tags(&mut conn, jazz, 9999).is_err());
}

#[test]
fn batch_add_remove_is_incremental_and_validated() {
    let mut conn = setup();
    let item1 = seed_item(&conn, "a.flac");
    let item2 = seed_item(&conn, "b.flac");

    let written = tags::add_tags_to_items(
        &mut conn,
        &[item1, item2],
        &["4K".to_string(), "HDR".to_string()],
    )
    .expect("batch");
    assert_eq!(written.len(), 4);
    assert_eq!(tag_count(&conn), 2);

    // 增量：已有标签重复打标返回空（幂等）
    let again = tags::add_tags_to_items(&mut conn, &[item1], &["4K".to_string()]).expect("idem");
    assert!(again.is_empty());
    // 只影响指定条目
    assert_eq!(link_count(&conn, item1), 2);
    assert_eq!(link_count(&conn, item2), 2);

    // 不存在的条目整体报错
    assert!(tags::add_tags_to_items(&mut conn, &[9999], &["x".to_string()]).is_err());

    // 移除
    let four_k: i64 = conn
        .query_row("SELECT id FROM tags WHERE name = '4K'", [], |r| r.get(0))
        .expect("4k id");
    let touched = tags::remove_tags_from_items(&conn, &[item1, item2], &[four_k]).expect("remove");
    assert_eq!(touched.len(), 2);
    assert_eq!(link_count(&conn, item1), 1);
    // 词表不删
    assert_eq!(tag_count(&conn), 2);
    // 再删一次无变更
    assert!(tags::remove_tags_from_items(&conn, &[item1], &[four_k])
        .expect("no-op")
        .is_empty());
}

#[test]
fn tag_changes_reindex_fts() {
    let mut conn = setup();
    // 扫描种子文件
    let root_dir = std::env::temp_dir().join(format!("vinecurator_tag_fts_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root_dir);
    fs::create_dir_all(root_dir.join("music")).expect("mkdir");
    fs::write(root_dir.join("music/Kind of Blue.flac"), b"a").expect("write");
    let root_id = vine_curator_lib::db::repo::add_root(&mut conn, &root_dir.to_string_lossy(), "t")
        .expect("root")
        .id;
    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut conn, root_id, &root_dir, &cancel, &mut |_| {}).expect("scan");

    let item = conn
        .query_row("SELECT id FROM items LIMIT 1", [], |r| r.get::<_, i64>(0))
        .expect("item");

    // 打标后 FTS 可按标签命中
    let written =
        tags::add_tags_to_items(&mut conn, &[item], &["Neuromancer".to_string()]).expect("tag");
    assert_eq!(written.len(), 1);
    fts::sync_item(&conn, item).expect("sync");
    let hits = fts::search_ids(&conn, "Neuromancer", 10).expect("search");
    assert_eq!(hits, vec![item]);

    // 移除标签后不再命中
    let tag_id = written[0].tag_id;
    tags::remove_tags_from_items(&conn, &[item], &[tag_id]).expect("remove");
    fts::sync_item(&conn, item).expect("sync after remove");
    assert!(fts::search_ids(&conn, "Neuromancer", 10)
        .expect("search2")
        .is_empty());
}
