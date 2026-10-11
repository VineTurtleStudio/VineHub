use rusqlite::Connection;
use vine_curator_lib::categories;
use vine_curator_lib::db;

fn setup() -> Connection {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_category_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("开库");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    conn
}

fn path_of(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT path FROM categories WHERE id = ?1", [id], |r| {
        r.get(0)
    })
    .expect("path")
}

fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
        .expect("count")
}

#[test]
fn create_builds_materialized_paths() {
    let mut conn = setup();
    let music = categories::create_category(&mut conn, None, "音乐").expect("root");
    let jazz = categories::create_category(&mut conn, Some(music), "爵士").expect("child");
    let bebop = categories::create_category(&mut conn, Some(jazz), "比波普").expect("grandchild");

    assert_eq!(path_of(&conn, music), "音乐");
    assert_eq!(path_of(&conn, jazz), "音乐/爵士");
    assert_eq!(path_of(&conn, bebop), "音乐/爵士/比波普");

    // 同级重名拒绝（music 下再建一个 "爵士"）
    assert!(categories::create_category(&mut conn, Some(music), "爵士").is_err());
    // 异级同名允许（"音乐" 下再建一个 "音乐" → "音乐/音乐"）
    assert!(categories::create_category(&mut conn, Some(music), "音乐").is_ok());
    // 不同父级下同名允许
    let film = categories::create_category(&mut conn, None, "音乐2").expect("sibling root");
    assert!(categories::create_category(&mut conn, Some(film), "爵士").is_ok());
    // 不存在的父级
    assert!(categories::create_category(&mut conn, Some(9999), "x").is_err());
    // 非法名
    assert!(categories::create_category(&mut conn, None, "a/b").is_err());
    assert!(categories::create_category(&mut conn, None, "  ").is_err());
}

#[test]
fn rename_rewrites_subtree_paths() {
    let mut conn = setup();
    let music = categories::create_category(&mut conn, None, "音乐").expect("music");
    let jazz = categories::create_category(&mut conn, Some(music), "爵士").expect("jazz");
    let bebop = categories::create_category(&mut conn, Some(jazz), "比波普").expect("bebop");

    categories::rename_category(&mut conn, jazz, "Jazz").expect("rename");
    assert_eq!(path_of(&conn, jazz), "音乐/Jazz");
    assert_eq!(path_of(&conn, bebop), "音乐/Jazz/比波普");
    // 改名后 name 同步
    let name: String = conn
        .query_row("SELECT name FROM categories WHERE id = ?1", [jazz], |r| {
            r.get(0)
        })
        .expect("name");
    assert_eq!(name, "Jazz");

    // 同级重名：music 下已存在 "Jazz"，再建一个同级改名应报错
    let classical = categories::create_category(&mut conn, Some(music), "古典").expect("古典");
    assert!(categories::rename_category(&mut conn, classical, "Jazz").is_err());
    assert!(categories::rename_category(&mut conn, classical, "Classical").is_ok());
    // 不存在的 id
    assert!(categories::rename_category(&mut conn, 9999, "x").is_err());
}

#[test]
fn move_rewrites_paths_and_blocks_cycles() {
    let mut conn = setup();
    let a = categories::create_category(&mut conn, None, "A").expect("a");
    let b = categories::create_category(&mut conn, None, "B").expect("b");
    let a1 = categories::create_category(&mut conn, Some(a), "1").expect("a1");
    let a1x = categories::create_category(&mut conn, Some(a1), "x").expect("a1x");

    // A 挂到 B 下
    categories::move_category(&mut conn, a, Some(b)).expect("move");
    assert_eq!(path_of(&conn, a), "B/A");
    assert_eq!(path_of(&conn, a1), "B/A/1");
    assert_eq!(path_of(&conn, a1x), "B/A/1/x");
    // parent_id 同步
    let pid: i64 = conn
        .query_row("SELECT parent_id FROM categories WHERE id = ?1", [a], |r| {
            r.get(0)
        })
        .expect("pid");
    assert_eq!(pid, b);

    // 防环：A 移到自己的子孙下
    assert!(categories::move_category(&mut conn, a, Some(a1)).is_err());
    // 移到自身
    assert!(categories::move_category(&mut conn, a, Some(a)).is_err());
    // 目标同级重名：B 下已有 "1"，把 a1（名 "1"）移进去应被拒
    let _b1 = categories::create_category(&mut conn, Some(b), "1").expect("b/1");
    assert!(categories::move_category(&mut conn, a1, Some(b)).is_err());
    // 原树未被改动
    assert_eq!(path_of(&conn, a1), "B/A/1");
    // 移到顶层
    categories::move_category(&mut conn, a, None).expect("to top");
    assert_eq!(path_of(&conn, a), "A");
    assert_eq!(path_of(&conn, a1), "A/1");
    assert_eq!(path_of(&conn, a1x), "A/1/x");
}

#[test]
fn move_detects_path_clash() {
    let mut conn = setup();
    let a = categories::create_category(&mut conn, None, "A").expect("a");
    let b = categories::create_category(&mut conn, None, "B").expect("b");
    let _ab = categories::create_category(&mut conn, Some(a), "B").expect("a/b");
    // B 移到 A 下 → 新路径 A/B 与已有 A/B 冲突
    assert!(categories::move_category(&mut conn, b, Some(a)).is_err());
    // 原树未被改动
    assert_eq!(path_of(&conn, b), "B");
    assert_eq!(count(&conn), 3);
}

#[test]
fn delete_cascades_subtree_and_links() {
    let mut conn = setup();
    let music = categories::create_category(&mut conn, None, "音乐").expect("music");
    let jazz = categories::create_category(&mut conn, Some(music), "爵士").expect("jazz");
    let _ = categories::create_category(&mut conn, Some(jazz), "比波普").expect("bebop");

    // 假条目挂分类
    conn.execute("INSERT INTO roots (path, label) VALUES ('/r', '')", [])
        .expect("root");
    conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name) VALUES (1, 'a.flac', 'a.flac')",
        [],
    )
    .expect("item");
    conn.execute(
        "INSERT INTO item_categories (item_id, category_id) VALUES (1, ?1)",
        [jazz],
    )
    .expect("link");

    let n = categories::delete_category(&conn, music).expect("delete");
    assert_eq!(n, 1);
    assert_eq!(count(&conn), 0);
    let links: i64 = conn
        .query_row("SELECT COUNT(*) FROM item_categories", [], |r| r.get(0))
        .expect("links");
    assert_eq!(links, 0);
    // 不存在的 id 报错
    assert!(categories::delete_category(&conn, 9999).is_err());
}

#[test]
fn name_validation_unit() {
    assert!(categories::validate_name("").is_err());
    assert!(categories::validate_name(" a/b ").is_err());
    assert_eq!(categories::validate_name(" 爵士 ").unwrap(), "爵士");
    assert!(categories::validate_name(&"长".repeat(101)).is_err());
    assert_eq!(categories::like_prefix("50%_x"), "50\\%\\_x%");
}

#[test]
fn injection_attempts_are_rejected_or_literal() {
    let mut conn = setup();
    // 分号在名称里合法，作为字面量存储（不执行）
    let id = categories::create_category(&mut conn, None, "x'; DROP TABLE categories;--")
        .expect("literal name");
    // 表仍在，名称按字面存储
    assert!(path_of(&conn, id).contains("DROP TABLE"));
    assert_eq!(count(&conn), 1);
    // 通配符名称不破坏 LIKE 查询
    let w = categories::create_category(&mut conn, None, "50%_off").expect("wild name");
    assert!(
        categories::create_category(&mut conn, None, "50%_off").is_err(),
        "重名仍被拦截"
    );
    assert_eq!(path_of(&conn, w), "50%_off");
}
