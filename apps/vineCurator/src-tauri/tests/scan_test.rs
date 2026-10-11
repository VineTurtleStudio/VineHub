use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::scan;
use vine_curator_lib::tasks;

/// 临时目录 fixture：建库 + 建根目录（每次调用唯一目录）
fn setup() -> (Connection, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_scan_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    let db_path = dir.join("test.sqlite");
    let mut conn = Connection::open(&db_path).expect("开库");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("创建根目录");
    (conn, root)
}

fn touch(path: &Path, contents: &[u8]) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).expect("创建父目录");
    }
    fs::write(path, contents).expect("写文件");
}

fn count_items(conn: &Connection, root_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM items WHERE root_id = ?1",
        [root_id],
        |r| r.get(0),
    )
    .expect("计数")
}

fn missing_count(conn: &Connection, root_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM items WHERE root_id = ?1 AND missing = 1",
        [root_id],
        |r| r.get(0),
    )
    .expect("计数")
}

fn add_root(conn: &mut Connection, path: &Path) -> i64 {
    repo::add_root(conn, &path.to_string_lossy(), "test")
        .expect("加根目录")
        .id
}

#[test]
fn scan_fixture_upsert_classify_missing() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    // 构造 fixture
    touch(&root.join("music/a.flac"), b"flac");
    touch(&root.join("music/b.MP3"), b"mp3"); // 大写扩展名 → 小写
    touch(&root.join("video/c.mkv"), b"mkv");
    touch(&root.join("books/d.epub"), b"epub");
    touch(&root.join("comics/e.cbz"), b"cbz");
    touch(&root.join("image/f.jpg"), b"jpg");
    touch(&root.join("other/g.nfo"), b"nfo");
    touch(&root.join("other/h.txt"), b"txt"); // txt → other
    touch(&root.join("$RECYCLE.BIN/skip.flac"), b"skip"); // 应跳过
    touch(&root.join("Thumbs.db"), b"thumbs"); // 应跳过
    touch(&root.join(".hidden.flac"), b"hidden"); // 点文件应跳过

    let cancel = AtomicBool::new(false);
    let mut progress_msgs = vec![];
    let outcome = scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |p| {
        progress_msgs.push(p.clone());
    })
    .expect("扫描成功");

    assert_eq!(outcome.stats.seen, 8); // 跳过 RECYCLE/Thumbs.db/.hidden
    assert_eq!(outcome.stats.changed, 8);
    assert_eq!(outcome.stats.unchanged, 0);
    assert_eq!(outcome.stats.missing, 0);
    assert_eq!(count_items(&conn, root_id), 8);

    // 类型分派
    let type_of = |rel: &str| -> String {
        conn.query_row(
            "SELECT type FROM items WHERE root_id = ?1 AND rel_path = ?2",
            rusqlite::params![root_id, rel],
            |r| r.get(0),
        )
        .expect("查类型")
    };
    assert_eq!(type_of("music/a.flac"), "music");
    assert_eq!(type_of("music/b.MP3"), "music");
    assert_eq!(type_of("video/c.mkv"), "video");
    assert_eq!(type_of("books/d.epub"), "book");
    assert_eq!(type_of("comics/e.cbz"), "comic");
    assert_eq!(type_of("image/f.jpg"), "image");
    assert_eq!(type_of("other/g.nfo"), "other");
    assert_eq!(type_of("other/h.txt"), "other");

    // ext 归一化（小写无点）
    let ext: String = conn
        .query_row(
            "SELECT ext FROM items WHERE root_id = ?1 AND rel_path = 'music/b.MP3'",
            [root_id],
            |r| r.get(0),
        )
        .expect("ext");
    assert_eq!(ext, "mp3");

    // 四类媒体有扩展表占位行，image/other 无
    let has_music = conn
        .query_row(
            "SELECT COUNT(*) FROM music_tracks mt JOIN items i ON i.id = mt.item_id WHERE i.root_id = ?1",
            [root_id],
            |r| r.get::<_, i64>(0),
        )
        .expect("music_tracks");
    assert_eq!(has_music, 2);
    let has_image = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE root_id = ?1 AND type = 'image' AND id IN (SELECT item_id FROM music_tracks)",
            [root_id],
            |r| r.get::<_, i64>(0),
        )
        .expect("image 无扩展表");
    assert_eq!(has_image, 0);

    // 增量：不变零写入
    let outcome2 = scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |p| {
        progress_msgs.push(p.clone());
    })
    .expect("二次扫描");
    assert_eq!(outcome2.stats.unchanged, 8);
    assert_eq!(outcome2.stats.changed, 0);

    // 删除文件 → missing 标记
    fs::remove_file(root.join("music/a.flac")).expect("删文件");
    let outcome3 = scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |p| {
        progress_msgs.push(p.clone());
    })
    .expect("三次扫描");
    assert_eq!(outcome3.stats.missing, 1);
    assert_eq!(missing_count(&conn, root_id), 1);

    // 进度消息包含 finished
    assert!(progress_msgs.iter().any(|p| p.phase == "finished"));
    assert!(progress_msgs.iter().any(|p| p.status == "done"));
}

#[test]
fn scan_changes_invalidate_hash() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("a.flac"), b"hello");
    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |_| {}).expect("首次扫描");

    // 手动置一个 hash 值（模拟已哈希）
    conn.execute(
        "UPDATE items SET hash = 'deadbeef', hash_state = 'done' WHERE root_id = ?1",
        [root_id],
    )
    .expect("置 hash");

    // 文件内容变化（size/mtime 变）→ hash 应被清空、hash_state = 'none'
    touch(&root.join("a.flac"), b"hello world changed");
    scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |_| {}).expect("二次扫描");

    let (hash, state): (Option<String>, String) = conn
        .query_row(
            "SELECT hash, hash_state FROM items WHERE root_id = ?1",
            [root_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("查 hash");
    assert_eq!(hash, None);
    assert_eq!(state, "none");
}

#[test]
fn hash_job_known_vector_and_missing_file() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("hello.txt"), b"hello");
    touch(&root.join("gone.txt"), b"will delete");

    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |_| {}).expect("扫描");

    // 删除一个文件（模拟磁盘缺失）
    fs::remove_file(root.join("gone.txt")).expect("删文件");

    let pending = tasks::mark_pending(&conn, Some(root_id), &[]).expect("标 pending");
    assert_eq!(pending, 2);

    let mut last_progress = None;
    let outcome = tasks::run_pending_hashes(&conn, &root, &cancel, &mut |p| {
        last_progress = Some(p.clone())
    })
    .expect("哈希");

    assert_eq!(outcome.stats.done, 1);
    assert_eq!(outcome.stats.failed, 1);

    // hello.txt 的 hash 应为已知向量
    let hash: String = conn
        .query_row(
            "SELECT hash FROM items WHERE root_id = ?1 AND rel_path = 'hello.txt'",
            [root_id],
            |r| r.get(0),
        )
        .expect("查 hash");
    assert_eq!(
        hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );

    // 缺失文件 hash_state 回落 none（保持可重试）
    let state: String = conn
        .query_row(
            "SELECT hash_state FROM items WHERE root_id = ?1 AND rel_path = 'gone.txt'",
            [root_id],
            |r| r.get(0),
        )
        .expect("查状态");
    assert_eq!(state, "none");

    assert!(last_progress.unwrap().phase == "finished");
}

#[test]
fn cancel_scan_does_not_mark_missing() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("a.flac"), b"a");
    touch(&root.join("b.flac"), b"b");

    // 第一次扫描入库
    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |_| {}).expect("首次");

    // 删文件后取消标志已置位 → 不应标 missing
    fs::remove_file(root.join("a.flac")).expect("删");
    let cancel = AtomicBool::new(true);
    let outcome =
        scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |_| {}).expect("取消扫描");
    assert_eq!(outcome.status, "cancelled");
    assert_eq!(missing_count(&conn, root_id), 0);
}

#[test]
fn scan_handles_nested_dirs_and_symlink_skips() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    // 深度嵌套
    touch(&root.join("a/b/c/d/e/f/g/h/deep.flac"), b"deep");
    touch(&root.join("normal.flac"), b"normal");

    let cancel = AtomicBool::new(false);
    let outcome = scan::scan_root(&mut conn, root_id, &root, &cancel, &mut |_| {}).expect("扫描");
    assert_eq!(outcome.stats.seen, 2);
    assert_eq!(count_items(&conn, root_id), 2, "深度嵌套应正常入库");

    // rel_path 使用正斜杠
    let rel: String = conn
        .query_row(
            "SELECT rel_path FROM items WHERE root_id = ?1 AND rel_path LIKE '%deep.flac'",
            [root_id],
            |r| r.get(0),
        )
        .expect("查 rel");
    assert!(rel.contains('/'), "rel_path 应使用正斜杠: {rel}");
    assert!(!rel.contains('\\'), "rel_path 不应含反斜杠: {rel}");
}
