use rusqlite::Connection;
use vine_curator_lib::db::migrate::{applied_versions, migrate, MIGRATIONS};
use vine_curator_lib::db::{open_pool, repo};

const EXPECTED_TABLES: &[&str] = &[
    "schema_migrations",
    "settings",
    "roots",
    "items",
    "categories",
    "item_categories",
    "tags",
    "tag_aliases",
    "item_tags",
    "smart_collections",
    "rules",
    "jobs",
    "music_tracks",
    "video_items",
    "book_items",
    "comic_items",
    "fts_items",
    "users",
    "audit_log",
];

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

#[test]
fn migrate_full_run_and_idempotent() {
    let mut conn = Connection::open_in_memory().expect("open in-memory db");
    let newly = migrate(&mut conn).expect("first migrate");
    assert_eq!(
        newly,
        MIGRATIONS.iter().map(|m| m.version).collect::<Vec<_>>(),
        "首轮必须按序应用全部迁移"
    );

    let applied = applied_versions(&conn).expect("applied_versions");
    assert_eq!(applied, newly);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, MIGRATIONS.len() as i64);

    let again = migrate(&mut conn).expect("second migrate");
    assert!(again.is_empty(), "重复迁移必须幂等，实际={again:?}");
}

#[test]
fn migrate_creates_all_expected_tables() {
    let mut conn = Connection::open_in_memory().expect("open in-memory db");
    migrate(&mut conn).expect("migrate");
    for t in EXPECTED_TABLES {
        assert!(table_exists(&conn, t), "缺少表 {t}");
    }
}

#[test]
fn item_categories_has_source_column_default_manual() {
    let mut conn = Connection::open_in_memory().expect("open in-memory db");
    migrate(&mut conn).expect("migrate");
    conn.execute("INSERT INTO roots (path) VALUES ('D:/m')", [])
        .unwrap();
    let root = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name, type) VALUES (?1, 'a.flac', 'a.flac', 'music')",
        [root],
    )
    .unwrap();
    let item = conn.last_insert_rowid();
    conn.execute("INSERT INTO categories (name, path) VALUES ('X', 'X')", [])
        .unwrap();
    let cat = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO item_categories (item_id, category_id) VALUES (?1, ?2)",
        rusqlite::params![item, cat],
    )
    .unwrap();
    let src: String = conn
        .query_row(
            "SELECT source FROM item_categories WHERE item_id = ?1",
            [item],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(src, "manual", "0008 默认 source=manual");
}

#[test]
fn items_schema_enforces_domain_checks() {
    let mut conn = Connection::open_in_memory().expect("open in-memory db");
    migrate(&mut conn).expect("migrate");

    conn.execute("INSERT INTO roots (path) VALUES ('D:/media')", [])
        .expect("insert root");
    let root_id = conn.last_insert_rowid();

    let bad_type = conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name, type) VALUES (?1, 'a.flac', 'a.flac', 'nope')",
        [root_id],
    );
    assert!(bad_type.is_err(), "type 不在枚举内必须被拒绝");

    let bad_rating = conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name, rating) VALUES (?1, 'b.flac', 'b.flac', 9)",
        [root_id],
    );
    assert!(bad_rating.is_err(), "rating 越界必须被拒绝");

    conn.execute(
        "INSERT INTO items (root_id, rel_path, file_name, type) VALUES (?1, 'c.mp3', 'c.mp3', 'music')",
        [root_id],
    )
    .expect("合法插入");
}

#[test]
fn fts5_roundtrip() {
    let mut conn = Connection::open_in_memory().expect("open in-memory db");
    migrate(&mut conn).expect("migrate");

    conn.execute(
        "INSERT INTO fts_items (item_id, title, body, tags_text) VALUES (1, 'Kind of Blue', 'Miles Davis album', 'jazz')",
        [],
    )
    .expect("fts insert");

    let hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM fts_items WHERE fts_items MATCH 'jazz'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1, "FTS5 MATCH 必须命中");

    let miss: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM fts_items WHERE fts_items MATCH 'blues'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(miss, 0);
}

#[test]
fn pool_wal_settings_roundtrip() {
    let path = std::env::temp_dir().join(format!(
        "vinecurator-migrate-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let pool = open_pool(&path).expect("open pool");
    {
        let mut conn = pool.get().expect("get conn");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "必须启用 WAL");

        migrate(&mut conn).expect("migrate via pool");

        let entries = vec![
            repo::SettingEntry {
                key: "ffprobe_path".into(),
                value: "C:/tools/ffprobe.exe".into(),
            },
            repo::SettingEntry {
                key: "derived_dir".into(),
                value: "D:/derived".into(),
            },
        ];
        repo::save_settings(&mut conn, &entries).expect("save");

        let loaded = repo::get_settings(&conn).expect("get");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].key, "derived_dir");
        assert_eq!(loaded[0].value, "D:/derived");

        repo::save_settings(
            &mut conn,
            &[repo::SettingEntry {
                key: "derived_dir".into(),
                value: "E:/derived2".into(),
            }],
        )
        .expect("upsert");
        let loaded = repo::get_settings(&conn).expect("get2");
        assert_eq!(loaded.len(), 2, "upsert 不得新增行");
        assert_eq!(
            loaded
                .iter()
                .find(|e| e.key == "derived_dir")
                .unwrap()
                .value,
            "E:/derived2"
        );
    }
    drop(pool);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
