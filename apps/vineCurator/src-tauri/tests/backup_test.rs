use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use vine_curator_lib::backup::{self, ResticRunner};
use vine_curator_lib::db;
use vine_curator_lib::db::repo::{self, SettingEntry};

fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_backup_{tag}_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("临时目录");
    dir
}

fn setup() -> (Connection, PathBuf) {
    let dir = temp_dir("db");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("打开");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    repo::save_settings(
        &mut conn,
        &[SettingEntry {
            key: "derived_dir".into(),
            value: String::new(),
        }],
    )
    .expect("seed settings");
    (conn, dir)
}

fn count_snapshots(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("catalog-") && n.ends_with(".sqlite")
                })
                .count()
        })
        .unwrap_or(0)
}

struct FakeRestic {
    fail: bool,
    calls: Mutex<Vec<String>>,
}

impl FakeRestic {
    fn new(fail: bool) -> Self {
        Self {
            fail,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl ResticRunner for FakeRestic {
    fn backup(&self, dir: &Path, repo_url: &str, _password_file: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push(repo_url.to_string());
        if self.fail {
            Err("fake restic 失败".into())
        } else {
            Ok(format!("saved snapshot of {}", dir.display()))
        }
    }
}

#[test]
fn snapshot_creates_queryable_copy_and_prunes() {
    let (mut conn, dir) = setup();
    repo::save_settings(
        &mut conn,
        &[SettingEntry {
            key: "marker".into(),
            value: "hello-snapshot".into(),
        }],
    )
    .expect("seed");
    let bdir = dir.join("bk");

    // 三份快照，只保留最新 1 份
    let p1 = backup::snapshot(&conn, &bdir, 1).expect("snap1");
    assert!(p1.exists());
    assert_eq!(count_snapshots(&bdir), 1);
    let _p2 = backup::snapshot(&conn, &bdir, 1).expect("snap2");
    let p3 = backup::snapshot(&conn, &bdir, 1).expect("snap3");
    assert_eq!(count_snapshots(&bdir), 1, "keep=1 只留最新");
    assert!(p3.exists(), "最新快照保留");

    // 快照是可独立打开的有效 SQLite，数据在
    let marker: String = {
        let copy = Connection::open(&p3).expect("open copy");
        copy.query_row("SELECT value FROM settings WHERE key = 'marker'", [], |r| {
            r.get(0)
        })
        .expect("marker")
    };
    assert_eq!(marker, "hello-snapshot");

    // keep=2 → 两份
    let _ = backup::snapshot(&conn, &bdir, 2).expect("snap4");
    let _ = backup::snapshot(&conn, &bdir, 2).expect("snap5");
    assert!(count_snapshots(&bdir) <= 2, "keep=2 最多两份");
}

#[test]
fn run_backup_restic_fallback_and_last_backup_at() {
    let (mut conn, dir) = setup();
    let default_derived = dir.join("derived");
    let bdir = dir.join("bk2");
    repo::save_settings(
        &mut conn,
        &[
            SettingEntry {
                key: "backup_dir".into(),
                value: bdir.to_string_lossy().into_owned(),
            },
            SettingEntry {
                key: "backup_keep".into(),
                value: "3".into(),
            },
            SettingEntry {
                key: "restic_repo".into(),
                value: "sftp:backup@nas:/vb".into(),
            },
        ],
    )
    .expect("seed");

    // restic 成功
    let fake = FakeRestic::new(false);
    let report = backup::run_backup(&mut conn, &default_derived, Some(&fake)).expect("run");
    assert!(report.snapshot_path.contains("bk2"));
    assert!(report.restic.is_some());
    assert!(report.restic_skipped.is_none());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert!(Path::new(&report.snapshot_path).exists());

    // last_backup_at = epoch ms
    let last: String = repo::get_setting_value(&conn, "last_backup_at").expect("last");
    let last_ms: u128 = last.parse().expect("ms");
    assert!(last_ms > 0);

    // restic 失败 → 降级但快照保留
    let failing = FakeRestic::new(true);
    let report = backup::run_backup(&mut conn, &default_derived, Some(&failing)).expect("run2");
    assert!(report.restic.is_none());
    assert!(
        report
            .restic_skipped
            .as_deref()
            .unwrap_or_default()
            .contains("失败"),
        "skipped={:?}",
        report.restic_skipped
    );
    assert!(Path::new(&report.snapshot_path).exists());
    assert_eq!(failing.calls.lock().unwrap().len(), 1);

    // 未配置 restic_repo → 跳过且不调 runner
    repo::save_settings(
        &mut conn,
        &[SettingEntry {
            key: "restic_repo".into(),
            value: String::new(),
        }],
    )
    .expect("clear repo");
    let unused = FakeRestic::new(false);
    let report = backup::run_backup(&mut conn, &default_derived, Some(&unused)).expect("run3");
    assert!(report.restic.is_none());
    assert!(report
        .restic_skipped
        .as_deref()
        .unwrap_or_default()
        .contains("未配置"));
    assert_eq!(unused.calls.lock().unwrap().len(), 0, "未配置不调 restic");
}

#[test]
fn default_dir_resolution_and_keep_clamping() {
    let (mut conn, dir) = setup();
    let default_derived = dir.join("derived");

    // backup_dir 空 → default_derived/backups
    let report = backup::run_backup(&mut conn, &default_derived, None).expect("run");
    assert!(
        report.snapshot_path.contains("backups"),
        "path={}",
        report.snapshot_path
    );
    assert!(report
        .restic_skipped
        .as_deref()
        .unwrap_or("")
        .contains("未配置"));

    // keep 非法回 7、0 钳 1
    let cfg = backup::load_config(&conn);
    assert_eq!(cfg.keep, 7);
    repo::save_settings(
        &mut conn,
        &[SettingEntry {
            key: "backup_keep".into(),
            value: "0".into(),
        }],
    )
    .expect("seed");
    let cfg = backup::load_config(&conn);
    assert_eq!(cfg.keep, 1, "keep=0 钳到 1");
}
