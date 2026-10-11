use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::meta;
use vine_curator_lib::meta::beets;
use vine_curator_lib::meta::calibre;
use vine_curator_lib::meta::runner::Runner;
use vine_curator_lib::scan;

fn setup() -> (Connection, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_meta_test_{}_{}",
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

fn add_root(conn: &mut Connection, path: &Path) -> i64 {
    repo::add_root(conn, &path.to_string_lossy(), "test")
        .expect("加根目录")
        .id
}

fn scan_all(conn: &mut Connection, root_id: i64, root: &Path) {
    let cancel = AtomicBool::new(false);
    scan::scan_root(conn, root_id, root, &cancel, &mut |_| {}).expect("扫描");
}

fn meta_state(conn: &Connection, rel: &str) -> String {
    conn.query_row(
        "SELECT meta_state FROM items WHERE rel_path = ?1",
        [rel],
        |r| r.get(0),
    )
    .expect("查 meta_state")
}

struct FakeRunner;

impl Runner for FakeRunner {
    fn probe(&mut self, abs_path: &Path, type_name: &str) -> Result<String, String> {
        let name = abs_path.to_string_lossy();
        if name.contains("broken") {
            return Err("探测失败".into());
        }
        match type_name {
            "video" => Ok(r#"{"format":{"duration":"120.5","bit_rate":"4500000","format_name":"matroska,webm","tags":{"date":"2022-01-02"}},"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080}]}"#.into()),
            "music" => Ok(r#"{"format":{"duration":"200.0","bit_rate":"900000","format_name":"flac","tags":{"ARTIST":"Test Artist","ALBUM":"Test Album","TRACK":"2/10","YEAR":"2020"}}}"#.into()),
            "book" => Ok(r#"[{"Title":"T","Author":"Book Author","ISBN":"978-1","Publisher":"Pub","PageCount":300,"PublishDate":"2019/01/01","FileTypeExtension":"epub"}]"#.into()),
            "comic" => Ok(r#"[{"Series":"S1","Volume":"3","PageCount":24}]"#.into()),
            other => Err(format!("不支持类型: {other}")),
        }
    }
}

#[test]
fn probe_flow_fills_tables_and_states() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("video/movie.mkv"), b"mkv");
    touch(&root.join("music/track.flac"), b"flac");
    touch(&root.join("books/novel.epub"), b"epub");
    touch(&root.join("comics/issue.cbz"), b"cbz");
    touch(&root.join("other/note.txt"), b"txt"); // 非四类，不探测
    scan_all(&mut conn, root_id, &root);

    // 待探测 = 四类
    let pending = meta::pending_probe(&conn, None).expect("pending");
    assert_eq!(pending.len(), 4);

    let cancel = AtomicBool::new(false);
    let mut msgs = vec![];
    let mut runner = FakeRunner;
    let outcome = meta::probe_root(&mut conn, None, &mut runner, &cancel, &mut |p| {
        msgs.push(p.clone());
    })
    .expect("探测");
    assert_eq!(outcome.status, "done");
    assert_eq!(outcome.stats.probed, 4);
    assert_eq!(outcome.stats.failed, 0);
    assert_eq!(outcome.stats.skipped, 0);

    // 状态 + metadata JSON
    for rel in [
        "video/movie.mkv",
        "music/track.flac",
        "books/novel.epub",
        "comics/issue.cbz",
    ] {
        assert_eq!(meta_state(&conn, rel), "done", "{rel}");
        let meta_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM items WHERE rel_path = ?1",
                [rel],
                |r| r.get(0),
            )
            .expect("metadata");
        assert!(meta_json.is_some(), "{rel} 应落 metadata JSON");
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM items WHERE type = 'other'", [], |r| r
            .get::<_, i64>(0))
            .expect("other"),
        1
    );

    // 扩展表回填
    let (res, codec, dur): (Option<String>, Option<String>, Option<f64>) = conn
        .query_row(
            "SELECT v.resolution, v.codec, v.duration FROM video_items v
             JOIN items i ON i.id = v.item_id WHERE i.rel_path = 'video/movie.mkv'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("video_items");
    assert_eq!(res.as_deref(), Some("1920x1080"));
    assert_eq!(codec.as_deref(), Some("h264"));
    assert_eq!(dur, Some(120.5));

    let (artist, album, track): (Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT m.artist, m.album, m.track FROM music_tracks m
             JOIN items i ON i.id = m.item_id WHERE i.rel_path = 'music/track.flac'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("music_tracks");
    assert_eq!(artist.as_deref(), Some("Test Artist"));
    assert_eq!(album.as_deref(), Some("Test Album"));
    assert_eq!(track, Some(2));

    let (author, isbn): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT b.author, b.isbn FROM book_items b
             JOIN items i ON i.id = b.item_id WHERE i.rel_path = 'books/novel.epub'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("book_items");
    assert_eq!(author.as_deref(), Some("Book Author"));
    assert_eq!(isbn.as_deref(), Some("978-1"));

    let (series, volume, pages): (Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT c.series, c.volume, c.page_count FROM comic_items c
             JOIN items i ON i.id = c.item_id WHERE i.rel_path = 'comics/issue.cbz'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("comic_items");
    assert_eq!(series.as_deref(), Some("S1"));
    assert_eq!(volume.as_deref(), Some("3"));
    assert_eq!(pages, Some(24));

    // year 回填
    let year: Option<i64> = conn
        .query_row(
            "SELECT year FROM items WHERE rel_path = 'video/movie.mkv'",
            [],
            |r| r.get(0),
        )
        .expect("year");
    assert_eq!(year, Some(2022));

    // 进度含 finished/done
    assert!(msgs.iter().any(|p| p.phase == "finished"));
    assert!(msgs.iter().any(|p| p.status == "done"));

    // 二次探测：全部已 done → pending 为 0
    let pending2 = meta::pending_probe(&conn, None).expect("pending2");
    assert!(pending2.is_empty());
}

#[test]
fn probe_failure_marks_failed_and_skips_missing() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("video/broken.mkv"), b"broken");
    touch(&root.join("music/ok.flac"), b"ok");
    touch(&root.join("video/gone.mkv"), b"gone");
    scan_all(&mut conn, root_id, &root);

    // 磁盘缺失（未重扫，missing=0 但文件不存在）
    fs::remove_file(root.join("video/gone.mkv")).expect("删文件");

    let cancel = AtomicBool::new(false);
    let mut runner = FakeRunner;
    let outcome =
        meta::probe_root(&mut conn, None, &mut runner, &cancel, &mut |_| {}).expect("探测");
    assert_eq!(outcome.stats.probed, 1);
    assert_eq!(outcome.stats.failed, 1);
    assert_eq!(outcome.stats.skipped, 1);

    assert_eq!(meta_state(&conn, "video/broken.mkv"), "failed");
    assert_eq!(meta_state(&conn, "music/ok.flac"), "done");
    // 缺失文件保持 none，可重试
    assert_eq!(meta_state(&conn, "video/gone.mkv"), "none");

    // failed 可重新探测（不在 pending 里 → 需手动重置）
    let pending = meta::pending_probe(&conn, None).expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].rel_path, "video/gone.mkv");
}

#[test]
fn probe_cancel_stops_and_keeps_states() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("music/a.flac"), b"a");
    touch(&root.join("music/b.flac"), b"b");
    scan_all(&mut conn, root_id, &root);

    let cancel = AtomicBool::new(true);
    let mut runner = FakeRunner;
    let mut msgs = vec![];
    let outcome = meta::probe_root(&mut conn, None, &mut runner, &cancel, &mut |p| {
        msgs.push(p.clone());
    })
    .expect("取消探测");
    assert_eq!(outcome.status, "cancelled");
    assert_eq!(outcome.stats.probed, 0);
    // 取消不改状态
    assert_eq!(meta_state(&conn, "music/a.flac"), "none");
    assert!(msgs.iter().any(|p| p.status == "cancelled"));
}

#[test]
fn root_scope_limits_probe() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);
    let root2 = root.with_file_name("root2");
    fs::create_dir_all(&root2).expect("root2");
    let root2_id = add_root(&mut conn, &root2);

    touch(&root.join("music/a.flac"), b"a");
    touch(&root2.join("music/b.flac"), b"b");
    scan_all(&mut conn, root_id, &root);
    scan_all(&mut conn, root2_id, &root2);

    let cancel = AtomicBool::new(false);
    let mut runner = FakeRunner;
    let outcome = meta::probe_root(&mut conn, Some(root_id), &mut runner, &cancel, &mut |_| {})
        .expect("限定根探测");
    assert_eq!(outcome.stats.probed, 1);
    assert_eq!(meta_state(&conn, "music/a.flac"), "done");
    assert_eq!(meta_state(&conn, "music/b.flac"), "none");
}

#[test]
fn beets_import_matches_absolute_paths() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    touch(&root.join("music/artist/song.flac"), b"song");
    touch(&root.join("music/other.flac"), b"other");
    scan_all(&mut conn, root_id, &root);

    // 构造 beets 库 fixture
    let base = std::env::temp_dir().join(format!("vinecurator_beets_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("beets dir");
    let db_path = base.join("library.db");
    let remote = Connection::open(&db_path).expect("beets 库");
    remote
        .execute_batch(
            "CREATE TABLE items (
               path TEXT, artist TEXT, albumartist TEXT, album TEXT,
               disc INTEGER, track INTEGER, length REAL, format TEXT,
               bitrate INTEGER, mb_trackid TEXT, year INTEGER
             );",
        )
        .expect("建表");
    let matched_abs = root.join("music/artist/song.flac");
    remote
        .execute(
            "INSERT INTO items (path, artist, album, track, length, format, bitrate, year)
             VALUES (?1, 'Beets Artist', 'Beets Album', 7, 180.0, 'FLAC', 1000000, 2015)",
            [&matched_abs.to_string_lossy()],
        )
        .expect("插行");
    remote
        .execute(
            "INSERT INTO items (path, artist, album) VALUES (?1, 'X', 'Y')",
            ["D:/nowhere/ghost.flac"],
        )
        .expect("插行2");

    let cancel = AtomicBool::new(false);
    let mut msgs = vec![];
    let outcome =
        beets::import(&mut conn, &db_path, &cancel, &mut |p| msgs.push(p.clone())).expect("导入");
    assert_eq!(outcome.status, "done");
    assert_eq!(outcome.stats.imported, 1);
    assert_eq!(outcome.stats.skipped, 1);

    type BeetsFields = (
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<f64>,
        Option<String>,
    );
    let (artist, album, track, duration, format): BeetsFields = conn
        .query_row(
            "SELECT m.artist, m.album, m.track, m.duration, m.format FROM music_tracks m
             JOIN items i ON i.id = m.item_id WHERE i.rel_path = 'music/artist/song.flac'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("music_tracks");
    assert_eq!(artist.as_deref(), Some("Beets Artist"));
    assert_eq!(album.as_deref(), Some("Beets Album"));
    assert_eq!(track, Some(7));
    assert_eq!(duration, Some(180.0));
    assert_eq!(format.as_deref(), Some("FLAC"));

    let year: Option<i64> = conn
        .query_row(
            "SELECT year FROM items WHERE rel_path = 'music/artist/song.flac'",
            [],
            |r| r.get(0),
        )
        .expect("year");
    assert_eq!(year, Some(2015));
    assert_eq!(meta_state(&conn, "music/artist/song.flac"), "done");
    assert!(msgs.iter().any(|p| p.phase == "finished"));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn calibre_import_builds_paths_and_matches() {
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);

    // 本库条目：root/books/Author/Title (dir)/Title.epub
    let cal_dir = root.join("books/Herbert/Dune (1)/Dune");
    touch(&cal_dir.with_extension("epub"), b"epub");
    scan_all(&mut conn, root_id, &root);

    // Calibre 书库 fixture：书库根 = 本根目录，books.path 相对书库根
    // 文件路径 = library_dir/books.path/data.name.<format 小写>
    let db_path = root.join("metadata.db");
    let remote = Connection::open(&db_path).expect("calibre 库");
    remote
        .execute_batch(
            "CREATE TABLE books (id INTEGER PRIMARY KEY, path TEXT, series_index REAL);
             CREATE TABLE data (book INTEGER, format TEXT, name TEXT);
             CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE books_authors_link (book INTEGER, author INTEGER);
             CREATE TABLE series (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE books_series_link (book INTEGER, series INTEGER);
             CREATE TABLE publishers (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE books_publishers_link (book INTEGER, publisher INTEGER);
             CREATE TABLE identifiers (book INTEGER, type TEXT, val TEXT);
             INSERT INTO books (id, path, series_index) VALUES (1, 'books/Herbert/Dune (1)', 1.0);
             INSERT INTO data (book, format, name) VALUES (1, 'EPUB', 'Dune');
             INSERT INTO authors (id, name) VALUES (1, 'Frank Herbert');
             INSERT INTO books_authors_link (book, author) VALUES (1, 1);
             INSERT INTO series (id, name) VALUES (1, 'Dune Saga');
             INSERT INTO books_series_link (book, series) VALUES (1, 1);
             INSERT INTO publishers (id, name) VALUES (1, 'Ace');
             INSERT INTO books_publishers_link (book, publisher) VALUES (1, 1);
             INSERT INTO identifiers (book, type, val) VALUES (1, 'isbn', '978-0441013593');",
        )
        .expect("calibre 建库");

    let cancel = AtomicBool::new(false);
    let outcome = calibre::import(&mut conn, &root, &cancel, &mut |_| {}).expect("导入");
    assert_eq!(outcome.status, "done");
    assert_eq!(outcome.stats.imported, 1);
    assert_eq!(outcome.stats.skipped, 0);

    type CalibreFields = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let (author, series, volume, isbn, publisher, format): CalibreFields = conn
        .query_row(
            "SELECT b.author, b.series, b.volume, b.isbn, b.publisher, b.format FROM book_items b
             JOIN items i ON i.id = b.item_id WHERE i.type = 'book'",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .expect("book_items");
    assert_eq!(author.as_deref(), Some("Frank Herbert"));
    assert_eq!(series.as_deref(), Some("Dune Saga"));
    assert_eq!(volume.as_deref(), Some("1"));
    assert_eq!(isbn.as_deref(), Some("978-0441013593"));
    assert_eq!(publisher.as_deref(), Some("Ace"));
    assert_eq!(format.as_deref(), Some("epub"));
    assert_eq!(
        meta_state(&conn, "books/Herbert/Dune (1)/Dune.epub"),
        "done"
    );
}

#[test]
fn normalize_key_and_scan_invalidates_meta_state() {
    // 键归一化（保留盘符冒号）
    assert_eq!(meta::normalize_key(r"D:\\Music\\a.flac"), "d:/music/a.flac");
    assert_eq!(meta::normalize_key("d:/music//a.flac"), "d:/music/a.flac");
    assert_eq!(meta::normalize_key("D:/MUSIC/A.FLAC"), "d:/music/a.flac");

    // 扫描变化重置 meta_state（0005 契约）
    let (mut conn, root) = setup();
    let root_id = add_root(&mut conn, &root);
    touch(&root.join("music/a.flac"), b"a");
    scan_all(&mut conn, root_id, &root);
    conn.execute(
        "UPDATE items SET meta_state = 'done' WHERE rel_path = 'music/a.flac'",
        [],
    )
    .expect("置 done");
    touch(&root.join("music/a.flac"), b"a changed content longer");
    scan_all(&mut conn, root_id, &root);
    assert_eq!(meta_state(&conn, "music/a.flac"), "none");
}
