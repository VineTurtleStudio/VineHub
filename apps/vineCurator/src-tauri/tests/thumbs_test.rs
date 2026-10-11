use rusqlite::Connection;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vine_curator_lib::db;
use vine_curator_lib::db::repo;
use vine_curator_lib::scan;
use vine_curator_lib::thumbs;

struct Fixture {
    conn: Connection,
    root: PathBuf,
    derived: PathBuf,
}

fn setup() -> Fixture {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_thumbs_test_{}_{}",
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
    let derived = dir.join("derived");
    Fixture {
        conn,
        root,
        derived,
    }
}

fn write_png(path: &Path, w: u32, h: u32) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).expect("父目录");
    }
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    image::DynamicImage::ImageRgb8(img)
        .save(path)
        .expect("写 PNG");
}

fn seed(fx: &mut Fixture) -> i64 {
    let root_id = repo::add_root(&mut fx.conn, &fx.root.to_string_lossy(), "t")
        .expect("加根目录")
        .id;
    write_png(&fx.root.join("pics/big.png"), 640, 480);
    write_png(&fx.root.join("pics/small.jpg"), 64, 64);
    fs::create_dir_all(fx.root.join("books")).expect("books 目录");
    fs::write(fx.root.join("books/a.epub"), b"epub").expect("epub");
    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut fx.conn, root_id, &fx.root, &cancel, &mut |_| {}).expect("扫描");
    root_id
}

fn item_id(conn: &Connection, name: &str) -> i64 {
    conn.query_row("SELECT id FROM items WHERE file_name = ?1", [name], |r| {
        r.get(0)
    })
    .expect("查 id")
}

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn image_thumb_resizes_freshens_and_marks_cover() {
    let mut fx = setup();
    seed(&mut fx);
    let id = item_id(&fx.conn, "big.png");

    let r = thumbs::ensure_thumb(&fx.conn, id, &fx.derived, "ffmpeg").expect("生成");
    assert!(r.created, "首次应生成");
    let path = PathBuf::from(&r.path);
    assert!(path.is_file());
    let img = image::open(&path).expect("解码 jpg");
    assert!(img.width() <= 320 && img.height() <= 320, "尺寸受限");

    let cover: i64 = fx
        .conn
        .query_row("SELECT has_cover FROM items WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .expect("has_cover");
    assert_eq!(cover, 1);

    let r2 = thumbs::ensure_thumb(&fx.conn, id, &fx.derived, "ffmpeg").expect("复用");
    assert!(!r2.created);
    assert_eq!(r2.path, r.path);

    let mut ov = HashMap::new();
    ov.insert(
        "ffmpeg_path".to_string(),
        "Z:\\nope\\ffmpeg.exe".to_string(),
    );
    assert_eq!(thumbs::ffmpeg_program(&ov), "Z:\\nope\\ffmpeg.exe");
    assert_eq!(thumbs::ffmpeg_program(&HashMap::new()), "ffmpeg");
}

#[test]
fn mtime_change_regenerates_and_cleans_stale() {
    let mut fx = setup();
    seed(&mut fx);
    let id = item_id(&fx.conn, "small.jpg");
    let r1 = thumbs::ensure_thumb(&fx.conn, id, &fx.derived, "ffmpeg").expect("gen1");
    assert!(r1.created);

    fx.conn
        .execute("UPDATE items SET mtime = mtime + 1000 WHERE id = ?1", [id])
        .expect("mtime");
    let r2 = thumbs::ensure_thumb(&fx.conn, id, &fx.derived, "ffmpeg").expect("gen2");
    assert!(r2.created, "mtime 变化应重新生成");
    assert_ne!(r1.path, r2.path);

    assert!(!PathBuf::from(&r1.path).is_file(), "旧文件应被清理");
    assert!(PathBuf::from(&r2.path).is_file());

    assert!(thumbs::thumb_exists(&fx.conn, id, &fx.derived).expect("exists"));
    let ex = thumbs::existing_thumb(&fx.conn, id, &fx.derived).expect("existing");
    assert_eq!(ex.as_deref(), Some(r2.path.as_str()));
}

#[test]
fn unsupported_type_and_missing_source_error() {
    let mut fx = setup();
    seed(&mut fx);
    let epub_id = item_id(&fx.conn, "a.epub");
    let err =
        thumbs::ensure_thumb(&fx.conn, epub_id, &fx.derived, "ffmpeg").expect_err("epub 不支持");
    assert!(err.contains("不支持"), "实际: {err}");

    let pic_id = item_id(&fx.conn, "big.png");
    fs::remove_file(fx.root.join("pics/big.png")).expect("删源");
    let err =
        thumbs::ensure_thumb(&fx.conn, pic_id, &fx.derived, "ffmpeg").expect_err("缺源应报错");
    assert!(err.contains("不存在"), "实际: {err}");

    let err = thumbs::ensure_thumb(&fx.conn, 999999, &fx.derived, "ffmpeg").expect_err("无条目");
    assert!(err.contains("条目不存在"), "实际: {err}");
}

#[test]
fn video_thumb_uses_ffmpeg_when_available() {
    let mut fx = setup();
    let vid = fx.root.join("video/clip.mp4");
    fs::create_dir_all(vid.parent().expect("p")).expect("dir");
    // 用 ffmpeg 造一个 2 秒红帧测试片；造不出来就跳过（不误报）
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x48:d=2",
            "-frames:v",
            "10",
            vid.to_str().expect("utf8"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made {
        eprintln!("跳过：本机 ffmpeg 无法生成测试视频");
        return;
    }
    assert!(ffmpeg_available(), "既然能造视频，ffmpeg 就在 PATH 上");
    seed(&mut fx);
    let id = item_id(&fx.conn, "clip.mp4");
    let r = thumbs::ensure_thumb(&fx.conn, id, &fx.derived, "ffmpeg").expect("抽帧");
    assert!(r.created);
    let img = image::open(&r.path).expect("抽帧产物可解码");
    assert!(img.width() <= 320);
    let cover: i64 = fx
        .conn
        .query_row("SELECT has_cover FROM items WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .expect("has_cover");
    assert_eq!(cover, 1);
}
