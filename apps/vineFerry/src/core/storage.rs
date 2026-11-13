use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::core::copy::{FileOutcome, FileStatus, RunSummary, TransferMode};
use crate::core::verify::{VerifyItem, VerifyStatus};

pub struct HistoryDb {
    conn: Connection,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunRecord {
    pub id: i64,
    pub ts: i64,
    pub label: String,
    pub mode: String,
    pub src: String,
    pub dst: String,
    pub total: u32,
    pub ok: u32,
    pub bad: u32,
    pub skipped: u32,
    pub cancelled: u32,
    pub bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyRunRecord {
    pub id: i64,
    pub ts: i64,
    pub algo: String,
    pub manifest_path: String,
    pub base: String,
    pub total: u32,
    pub ok: u32,
    pub missing: u32,
    pub mismatch: u32,
    pub read_error: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetRecord {
    pub id: i64,
    pub name: String,
    pub src: String,
    pub dst: String,
    pub mode: TransferMode,
    pub interval_secs: u64,
    pub enabled: bool,
    pub last_run: u64,
}

fn status_str(s: FileStatus) -> &'static str {
    match s {
        FileStatus::Verified => "verified",
        FileStatus::Failed => "failed",
        FileStatus::Skipped => "skipped",
        FileStatus::Cancelled => "cancelled",
    }
}

fn verify_status_str(s: VerifyStatus) -> &'static str {
    match s {
        VerifyStatus::Ok => "ok",
        VerifyStatus::Missing => "missing",
        VerifyStatus::Mismatch => "mismatch",
        VerifyStatus::ReadError => "read_error",
    }
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn default_db_path() -> PathBuf {
    crate::core::paths::data_dir().join("history.db")
}

impl HistoryDb {
    pub fn open_default() -> rusqlite::Result<Self> {
        Self::open(&default_db_path())
    }

    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS runs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts INTEGER NOT NULL,
                 label TEXT NOT NULL,
                 mode TEXT NOT NULL,
                 src TEXT NOT NULL,
                 dst TEXT NOT NULL,
                 total INTEGER NOT NULL,
                 ok INTEGER NOT NULL,
                 bad INTEGER NOT NULL,
                 skipped INTEGER NOT NULL,
                 cancelled INTEGER NOT NULL,
                 bytes INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS outcomes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                 rel TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 status TEXT NOT NULL,
                 verified INTEGER NOT NULL,
                 src_hash TEXT,
                 dst_hash TEXT,
                 message TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS verify_runs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts INTEGER NOT NULL,
                 algo TEXT NOT NULL,
                 manifest_path TEXT NOT NULL,
                 base TEXT NOT NULL,
                 total INTEGER NOT NULL,
                 ok INTEGER NOT NULL,
                 missing INTEGER NOT NULL,
                 mismatch INTEGER NOT NULL,
                 read_error INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS verify_items (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 run_id INTEGER NOT NULL REFERENCES verify_runs(id) ON DELETE CASCADE,
                 rel TEXT NOT NULL,
                 status TEXT NOT NULL,
                 expected TEXT,
                 actual TEXT,
                 message TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_outcomes_run ON outcomes(run_id);
             CREATE INDEX IF NOT EXISTS idx_verify_items_run ON verify_items(run_id);
             CREATE TABLE IF NOT EXISTS presets (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL UNIQUE,
                 src TEXT NOT NULL,
                 dst TEXT NOT NULL,
                 mode TEXT NOT NULL,
                 interval_secs INTEGER NOT NULL DEFAULT 0,
                 enabled INTEGER NOT NULL DEFAULT 1,
                 last_run INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        Ok(HistoryDb { conn })
    }

    pub fn insert_run(
        &self,
        label: &str,
        mode: TransferMode,
        src: &str,
        dst: &str,
        summary: &RunSummary,
    ) -> rusqlite::Result<i64> {
        let mode_str = match mode {
            TransferMode::Copy => "copy",
            TransferMode::Move => "move",
        };
        self.conn.execute(
            "INSERT INTO runs (ts, label, mode, src, dst, total, ok, bad, skipped, cancelled, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                now_ts(),
                label,
                mode_str,
                src,
                dst,
                summary.total,
                summary.ok,
                summary.bad,
                summary.skipped,
                summary.cancelled,
                summary.bytes as i64
            ],
        )?;
        let run_id = self.conn.last_insert_rowid();
        for o in &summary.outcomes {
            self.conn.execute(
                "INSERT INTO outcomes (run_id, rel, size, status, verified, src_hash, dst_hash, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    run_id,
                    o.rel,
                    o.size as i64,
                    status_str(o.status),
                    o.verified as i64,
                    o.src_hash,
                    o.dst_hash,
                    o.message
                ],
            )?;
        }
        Ok(run_id)
    }

    pub fn list_runs(&self, limit: u32) -> rusqlite::Result<Vec<RunRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, label, mode, src, dst, total, ok, bad, skipped, cancelled, bytes
             FROM runs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(RunRecord {
                id: r.get(0)?,
                ts: r.get(1)?,
                label: r.get(2)?,
                mode: r.get(3)?,
                src: r.get(4)?,
                dst: r.get(5)?,
                total: r.get::<_, i64>(6)? as u32,
                ok: r.get::<_, i64>(7)? as u32,
                bad: r.get::<_, i64>(8)? as u32,
                skipped: r.get::<_, i64>(9)? as u32,
                cancelled: r.get::<_, i64>(10)? as u32,
                bytes: r.get::<_, i64>(11)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn list_outcomes(&self, run_id: i64) -> rusqlite::Result<Vec<FileOutcome>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel, size, src_hash, dst_hash, verified, status, message
             FROM outcomes WHERE run_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![run_id], |r| {
            let status_s: String = r.get(5)?;
            let status = match status_s.as_str() {
                "verified" => FileStatus::Verified,
                "skipped" => FileStatus::Skipped,
                "cancelled" => FileStatus::Cancelled,
                _ => FileStatus::Failed,
            };
            Ok(FileOutcome {
                rel: r.get(0)?,
                size: r.get::<_, i64>(1)? as u64,
                src_hash: r.get(2)?,
                dst_hash: r.get(3)?,
                verified: r.get::<_, i64>(4)? != 0,
                status,
                message: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_verify_run(
        &self,
        algo: &str,
        manifest_path: &str,
        base: &str,
        items: &[VerifyItem],
    ) -> rusqlite::Result<i64> {
        let (ok, missing, mismatch, read_error) = crate::core::verify::summarize(items);
        self.conn.execute(
            "INSERT INTO verify_runs (ts, algo, manifest_path, base, total, ok, missing, mismatch, read_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                now_ts(),
                algo,
                manifest_path,
                base,
                items.len() as i64,
                ok as i64,
                missing as i64,
                mismatch as i64,
                read_error as i64
            ],
        )?;
        let run_id = self.conn.last_insert_rowid();
        for it in items {
            self.conn.execute(
                "INSERT INTO verify_items (run_id, rel, status, expected, actual, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    run_id,
                    it.path,
                    verify_status_str(it.status),
                    it.expected,
                    it.actual,
                    it.message
                ],
            )?;
        }
        Ok(run_id)
    }

    pub fn list_verify_runs(&self, limit: u32) -> rusqlite::Result<Vec<VerifyRunRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, algo, manifest_path, base, total, ok, missing, mismatch, read_error
             FROM verify_runs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(VerifyRunRecord {
                id: r.get(0)?,
                ts: r.get(1)?,
                algo: r.get(2)?,
                manifest_path: r.get(3)?,
                base: r.get(4)?,
                total: r.get::<_, i64>(5)? as u32,
                ok: r.get::<_, i64>(6)? as u32,
                missing: r.get::<_, i64>(7)? as u32,
                mismatch: r.get::<_, i64>(8)? as u32,
                read_error: r.get::<_, i64>(9)? as u32,
            })
        })?;
        rows.collect()
    }

    pub fn list_verify_items(&self, run_id: i64) -> rusqlite::Result<Vec<VerifyItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel, status, expected, actual, message FROM verify_items
             WHERE run_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![run_id], |r| {
            let status_s: String = r.get(1)?;
            let status = match status_s.as_str() {
                "ok" => VerifyStatus::Ok,
                "missing" => VerifyStatus::Missing,
                "mismatch" => VerifyStatus::Mismatch,
                _ => VerifyStatus::ReadError,
            };
            Ok(VerifyItem {
                path: r.get(0)?,
                status,
                expected: r.get(2)?,
                actual: r.get(3)?,
                message: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn upsert_preset(
        &self,
        name: &str,
        src: &str,
        dst: &str,
        mode: TransferMode,
        interval_secs: u64,
        enabled: bool,
    ) -> rusqlite::Result<()> {
        let mode_str = match mode {
            TransferMode::Copy => "copy",
            TransferMode::Move => "move",
        };
        self.conn.execute(
            "INSERT INTO presets (name, src, dst, mode, interval_secs, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(name) DO UPDATE SET
                 src = excluded.src, dst = excluded.dst, mode = excluded.mode,
                 interval_secs = excluded.interval_secs, enabled = excluded.enabled",
            params![name, src, dst, mode_str, interval_secs as i64, enabled as i64],
        )?;
        Ok(())
    }

    pub fn list_presets(&self) -> rusqlite::Result<Vec<PresetRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, src, dst, mode, interval_secs, enabled, last_run
             FROM presets ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            let mode_s: String = r.get(4)?;
            Ok(PresetRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                src: r.get(2)?,
                dst: r.get(3)?,
                mode: if mode_s == "move" {
                    TransferMode::Move
                } else {
                    TransferMode::Copy
                },
                interval_secs: r.get::<_, i64>(5)? as u64,
                enabled: r.get::<_, i64>(6)? != 0,
                last_run: r.get::<_, i64>(7)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn delete_preset(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM presets WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_preset_enabled(&self, id: i64, enabled: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE presets SET enabled = ?1 WHERE id = ?2",
            params![enabled as i64, id],
        )?;
        Ok(())
    }

    pub fn touch_preset(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE presets SET last_run = ?1 WHERE id = ?2",
            params![now_ts(), id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::copy::FileOutcome;

    fn sample_summary() -> RunSummary {
        RunSummary {
            total: 2,
            ok: 1,
            bad: 1,
            skipped: 0,
            cancelled: 0,
            bytes: 10,
            outcomes: vec![
                FileOutcome {
                    rel: "a.txt".into(),
                    size: 5,
                    src_hash: Some("aa".into()),
                    dst_hash: Some("aa".into()),
                    verified: true,
                    status: FileStatus::Verified,
                    message: "ok".into(),
                },
                FileOutcome {
                    rel: "b.txt".into(),
                    size: 5,
                    src_hash: None,
                    dst_hash: None,
                    verified: false,
                    status: FileStatus::Failed,
                    message: "boom".into(),
                },
            ],
        }
    }

    #[test]
    fn run_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let db = HistoryDb::open(&t.path().join("h.db")).unwrap();
        let id = db
            .insert_run("label", TransferMode::Copy, r"D:\src", r"D:\dst", &sample_summary())
            .unwrap();
        assert!(id > 0);
        let runs = db.list_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].total, 2);
        assert_eq!(runs[0].ok, 1);
        assert_eq!(runs[0].bad, 1);
        assert_eq!(runs[0].mode, "copy");
        let outcomes = db.list_outcomes(id).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].rel, "a.txt");
        assert!(outcomes[0].verified);
        assert_eq!(outcomes[1].status, FileStatus::Failed);
        assert_eq!(outcomes[1].message, "boom");
    }

    #[test]
    fn verify_run_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let db = HistoryDb::open(&t.path().join("h.db")).unwrap();
        let items = vec![
            VerifyItem {
                path: "a.txt".into(),
                status: VerifyStatus::Ok,
                expected: Some("aa".into()),
                actual: Some("aa".into()),
                message: "一致".into(),
            },
            VerifyItem {
                path: "b.txt".into(),
                status: VerifyStatus::Mismatch,
                expected: Some("bb".into()),
                actual: Some("cc".into()),
                message: "哈希不一致".into(),
            },
        ];
        let id = db
            .insert_verify_run("sha256", r"D:\m.sha256", r"D:\base", &items)
            .unwrap();
        assert!(id > 0);
        let runs = db.list_verify_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].total, 2);
        assert_eq!(runs[0].ok, 1);
        assert_eq!(runs[0].mismatch, 1);
        let loaded = db.list_verify_items(id).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].status, VerifyStatus::Mismatch);
        assert_eq!(loaded[1].actual.as_deref(), Some("cc"));
    }

    #[test]
    fn default_db_path_is_under_localappdata() {
        let p = default_db_path();
        assert!(p.to_string_lossy().to_lowercase().contains("vineferry"));
        assert!(p.ends_with("history.db"));
    }

    #[test]
    fn preset_roundtrip_update_delete() {
        let t = tempfile::tempdir().unwrap();
        let db = HistoryDb::open(&t.path().join("h.db")).unwrap();
        db.upsert_preset("nightly", r"D:\src", r"E:\dst", TransferMode::Copy, 3600, true)
            .unwrap();
        db.upsert_preset("nightly", r"D:\src2", r"E:\dst2", TransferMode::Move, 7200, false)
            .unwrap();
        let presets = db.list_presets().unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].src, r"D:\src2");
        assert_eq!(presets[0].dst, r"E:\dst2");
        assert_eq!(presets[0].mode, TransferMode::Move);
        assert_eq!(presets[0].interval_secs, 7200);
        assert!(!presets[0].enabled);
        db.touch_preset(presets[0].id).unwrap();
        let presets = db.list_presets().unwrap();
        assert!(presets[0].last_run > 0);
        db.set_preset_enabled(presets[0].id, true).unwrap();
        assert!(db.list_presets().unwrap()[0].enabled);
        db.delete_preset(presets[0].id).unwrap();
        assert!(db.list_presets().unwrap().is_empty());
    }
}
