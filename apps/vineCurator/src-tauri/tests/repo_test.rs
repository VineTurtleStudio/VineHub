use rusqlite::Connection;
use vine_curator_lib::db::migrate::migrate;
use vine_curator_lib::db::repo;

fn setup() -> Connection {
    let mut conn = Connection::open_in_memory().expect("open in-memory");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    migrate(&mut conn).expect("migrate");
    conn
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("vc-roots-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&p).expect("create temp dir");
    p
}

fn cleanup(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}

#[test]
fn add_and_list_root() {
    let mut conn = setup();
    let dir = temp_dir("add");
    let entry = repo::add_root(&mut conn, dir.to_str().unwrap(), "外置硬盘 A").expect("add");
    assert_eq!(entry.path, dir.to_str().unwrap());
    assert_eq!(entry.label, "外置硬盘 A");
    assert_eq!(entry.item_count, 0);

    let list = repo::list_roots(&conn).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, entry.id);
    cleanup(&dir);
}

#[test]
fn reject_missing_empty_and_non_dir() {
    let mut conn = setup();
    assert!(repo::add_root(&mut conn, "", "").is_err());
    assert!(repo::add_root(&mut conn, "C:\\vine-curator-no-such-dir-xyz", "").is_err());

    let file = temp_dir("file").join("a.txt");
    std::fs::write(&file, b"x").unwrap();
    assert!(repo::add_root(&mut conn, file.to_str().unwrap(), "").is_err());
    cleanup(file.parent().unwrap());
}

#[test]
fn reject_duplicate_and_nested_roots() {
    let mut conn = setup();
    let parent = temp_dir("nested");
    let child = parent.join("music");
    std::fs::create_dir_all(&child).unwrap();

    repo::add_root(&mut conn, parent.to_str().unwrap(), "").expect("add parent");
    assert!(
        repo::add_root(&mut conn, parent.to_str().unwrap(), "").is_err(),
        "完全重复必须拒绝"
    );
    let upper = parent.to_str().unwrap().to_uppercase();
    assert!(
        repo::add_root(&mut conn, &upper, "").is_err(),
        "大小写不同仍是同一目录"
    );
    assert!(
        repo::add_root(&mut conn, child.to_str().unwrap(), "").is_err(),
        "嵌套子目录必须拒绝"
    );
    assert!(
        repo::add_root(&mut conn, &format!("{}\\", parent.to_str().unwrap()), "").is_err(),
        "尾部分隔符等价"
    );

    let other = temp_dir("other");
    assert!(repo::add_root(&mut conn, other.to_str().unwrap(), "").is_ok());
    cleanup(&other);
    cleanup(&parent);
}

#[test]
fn redirect_updates_path_keeps_items() {
    let mut conn = setup();
    let a = temp_dir("redir-a");
    let b = temp_dir("redir-b");
    let c = temp_dir("redir-c");

    let root = repo::add_root(&mut conn, a.to_str().unwrap(), "").expect("add a");
    repo::add_root(&mut conn, b.to_str().unwrap(), "").expect("add b");
    conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name) VALUES (?1, 'x.mp3', 'x.mp3')",
        [root.id],
    )
    .expect("insert item");

    assert!(
        repo::redirect_root(&mut conn, root.id, b.to_str().unwrap()).is_err(),
        "重定向到已占用路径必须拒绝"
    );
    assert!(
        repo::redirect_root(&mut conn, 9999, c.to_str().unwrap()).is_err(),
        "不存在的 id 必须报错"
    );

    let updated = repo::redirect_root(&mut conn, root.id, c.to_str().unwrap()).expect("redirect");
    assert_eq!(updated.id, root.id);
    assert_eq!(updated.path, c.to_str().unwrap());
    assert_eq!(updated.item_count, 1, "重定向后索引条目保留");

    cleanup(&a);
    cleanup(&b);
    cleanup(&c);
}

#[test]
fn remove_root_cascades_index_only() {
    let mut conn = setup();
    let dir = temp_dir("remove");
    let root = repo::add_root(&mut conn, dir.to_str().unwrap(), "").expect("add");
    conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name) VALUES (?1, 'y.flac', 'y.flac')",
        [root.id],
    )
    .expect("insert item");
    conn.execute(
        "INSERT INTO music_tracks (item_id, artist) VALUES (1, 'Test')",
        [],
    )
    .expect("insert track");

    repo::remove_root(&mut conn, root.id).expect("remove");
    let items: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(items, 0, "删除根目录必须级联清掉索引条目");
    let tracks: i64 = conn
        .query_row("SELECT COUNT(*) FROM music_tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tracks, 0, "类型扩展表随 items 级联");
    let roots: i64 = conn
        .query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(roots, 0);
    assert!(
        std::fs::metadata(&dir).is_ok(),
        "磁盘目录与文件绝不能被动过"
    );
    assert!(
        repo::remove_root(&mut conn, root.id).is_err(),
        "二次删除报错"
    );
    cleanup(&dir);
}

#[test]
fn path_helpers_normalize_and_conflict() {
    assert_eq!(repo::normalize_root_path(" D:/Media/ "), "d:\\media");
    assert_eq!(repo::normalize_root_path("D:\\"), "d:\\");
    assert!(repo::paths_conflict("D:\\Media", "d:\\media\\"));
    assert!(repo::paths_conflict("D:\\Media\\music", "D:\\Media"));
    assert!(!repo::paths_conflict("D:\\Media", "E:\\Media"));
    assert!(!repo::paths_conflict("D:\\Media", "D:\\Media2"));
    assert!(!repo::paths_conflict("", ""));
}
