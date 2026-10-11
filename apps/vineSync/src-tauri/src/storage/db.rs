use crate::core::FileMeta;
use crate::model::{Counts, LogRow, PairStateDto};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sync_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    pair_id    TEXT NOT NULL,
    pair_name  TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT NOT NULL,
    status     TEXT NOT NULL,
    added      INTEGER NOT NULL DEFAULT 0,
    updated    INTEGER NOT NULL DEFAULT 0,
    deleted    INTEGER NOT NULL DEFAULT 0,
    versioned  INTEGER NOT NULL DEFAULT 0,
    skipped    INTEGER NOT NULL DEFAULT 0,
    failed     INTEGER NOT NULL DEFAULT 0,
    trigger_kind TEXT NOT NULL DEFAULT 'manual',
    message    TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_sync_log_id ON sync_log(id DESC);
CREATE TABLE IF NOT EXISTS pair_state (
    pair_id      TEXT PRIMARY KEY,
    pair_name    TEXT NOT NULL,
    last_sync_at TEXT,
    last_status  TEXT,
    added        INTEGER NOT NULL DEFAULT 0,
    updated      INTEGER NOT NULL DEFAULT 0,
    deleted      INTEGER NOT NULL DEFAULT 0,
    versioned    INTEGER NOT NULL DEFAULT 0,
    failed       INTEGER NOT NULL DEFAULT 0,
    message      TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS file_state (
    pair_id  TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    size     INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    PRIMARY KEY (pair_id, rel_path)
) WITHOUT ROWID;
"#;

pub struct NewLog {
    pub pair_id: String,
    pub pair_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub counts: Counts,
    pub trigger: String,
    pub message: String,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::msg!("创建数据库目录失败: {e}", "Failed to create database directory: {e}"))?;
        }
        let conn = Connection::open(path).map_err(|e| crate::msg!("打开数据库失败: {e}", "Failed to open database: {e}"))?;
        let _ = conn.busy_timeout(Duration::from_secs(5));
        conn.execute_batch(SCHEMA).map_err(|e| crate::msg!("初始化数据库失败: {e}", "Failed to init database: {e}"))?;
        Ok(Self { conn })
    }

    pub fn insert_log(&self, log: &NewLog) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO sync_log (pair_id, pair_name, started_at, finished_at, status,
                    added, updated, deleted, versioned, skipped, failed, trigger_kind, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    log.pair_id,
                    log.pair_name,
                    log.started_at,
                    log.finished_at,
                    log.status,
                    log.counts.added,
                    log.counts.updated,
                    log.counts.deleted,
                    log.counts.versioned,
                    log.counts.skipped,
                    log.counts.failed,
                    log.trigger,
                    log.message,
                ],
            )
            .map_err(|e| crate::msg!("写入同步日志失败: {e}", "Failed to write sync log: {e}"))?;
        Ok(())
    }

    pub fn history(&self, limit: u32) -> Result<Vec<LogRow>, String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT id, pair_id, pair_name, started_at, finished_at, status,
                        added, updated, deleted, versioned, skipped, failed, trigger_kind, message
                 FROM sync_log ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| crate::msg!("查询日志失败: {e}", "Failed to query log: {e}"))?;
        let rows = st
            .query_map(params![limit], |row| {
                Ok(LogRow {
                    id: row.get(0)?,
                    pair_id: row.get(1)?,
                    pair_name: row.get(2)?,
                    started_at: row.get(3)?,
                    finished_at: row.get(4)?,
                    status: row.get(5)?,
                    added: row.get(6)?,
                    updated: row.get(7)?,
                    deleted: row.get(8)?,
                    versioned: row.get(9)?,
                    skipped: row.get(10)?,
                    failed: row.get(11)?,
                    trigger: row.get(12)?,
                    message: row.get(13)?,
                })
            })
            .map_err(|e| crate::msg!("查询日志失败: {e}", "Failed to query log: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| crate::msg!("读取日志失败: {e}", "Failed to read log: {e}"))
    }

    pub fn clear_history(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM sync_log", [])
            .map_err(|e| crate::msg!("清空日志失败: {e}", "Failed to clear log: {e}"))?;
        Ok(())
    }

    pub fn upsert_pair_state(&self, st: &PairStateDto) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO pair_state (pair_id, pair_name, last_sync_at, last_status,
                    added, updated, deleted, versioned, failed, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(pair_id) DO UPDATE SET
                    pair_name = excluded.pair_name,
                    last_sync_at = excluded.last_sync_at,
                    last_status = excluded.last_status,
                    added = excluded.added,
                    updated = excluded.updated,
                    deleted = excluded.deleted,
                    versioned = excluded.versioned,
                    failed = excluded.failed,
                    message = excluded.message",
                params![
                    st.pair_id,
                    st.pair_name,
                    st.last_sync_at,
                    st.last_status,
                    st.added,
                    st.updated,
                    st.deleted,
                    st.versioned,
                    st.failed,
                    st.message,
                ],
            )
            .map_err(|e| crate::msg!("更新同步状态失败: {e}", "Failed to update sync state: {e}"))?;
        Ok(())
    }

    pub fn pair_state(&self, pair_id: &str) -> Result<Option<PairStateDto>, String> {
        self.conn
            .query_row(
                "SELECT pair_id, pair_name, last_sync_at, last_status,
                        added, updated, deleted, versioned, failed, message
                 FROM pair_state WHERE pair_id = ?1",
                params![pair_id],
                |row| {
                    Ok(PairStateDto {
                        pair_id: row.get(0)?,
                        pair_name: row.get(1)?,
                        last_sync_at: row.get(2)?,
                        last_status: row.get(3)?,
                        added: row.get(4)?,
                        updated: row.get(5)?,
                        deleted: row.get(6)?,
                        versioned: row.get(7)?,
                        failed: row.get(8)?,
                        message: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(|e| crate::msg!("查询同步状态失败: {e}", "Failed to query sync state: {e}"))
    }

    pub fn replace_file_state(
        &self,
        pair_id: &str,
        files: &BTreeMap<String, FileMeta>,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| crate::msg!("更新文件快照失败: {e}", "Failed to update file snapshot: {e}"))?;
        tx.execute("DELETE FROM file_state WHERE pair_id = ?1", params![pair_id])
            .map_err(|e| crate::msg!("更新文件快照失败: {e}", "Failed to update file snapshot: {e}"))?;
        {
            let mut st = tx
                .prepare(
                    "INSERT INTO file_state (pair_id, rel_path, size, mtime_ns)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| crate::msg!("更新文件快照失败: {e}", "Failed to update file snapshot: {e}"))?;
            for meta in files.values() {
                st.execute(params![pair_id, meta.rel, meta.size as i64, meta.mtime_ns])
                    .map_err(|e| crate::msg!("更新文件快照失败: {e}", "Failed to update file snapshot: {e}"))?;
            }
        }
        tx.commit().map_err(|e| crate::msg!("更新文件快照失败: {e}", "Failed to update file snapshot: {e}"))?;
        Ok(())
    }

    pub fn file_state_count(&self, pair_id: &str) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM file_state WHERE pair_id = ?1",
                params![pair_id],
                |row| row.get(0),
            )
            .map_err(|e| crate::msg!("查询文件快照失败: {e}", "Failed to query file snapshot: {e}"))
    }

    /// 读取某文件夹对的上次成功同步快照（key 为规范化相对路径）
    pub fn load_file_state(
        &self,
        pair_id: &str,
    ) -> Result<BTreeMap<String, FileMeta>, String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT rel_path, size, mtime_ns FROM file_state WHERE pair_id = ?1",
            )
            .map_err(|e| crate::msg!("查询文件快照失败: {e}", "Failed to query file snapshot: {e}"))?;
        let rows = st
            .query_map(params![pair_id], |row| {
                let rel: String = row.get(0)?;
                let size: i64 = row.get(1)?;
                let mtime_ns: i64 = row.get(2)?;
                Ok((rel, size, mtime_ns))
            })
            .map_err(|e| crate::msg!("查询文件快照失败: {e}", "Failed to query file snapshot: {e}"))?;
        let mut map = BTreeMap::new();
        for r in rows {
            let (rel, size, mtime_ns) =
                r.map_err(|e| crate::msg!("读取文件快照失败: {e}", "Read file snapshot failed: {e}"))?;
            let key = crate::core::normalize_key(&rel);
            map.insert(
                key,
                FileMeta { rel, size: size as u64, mtime_ns },
            );
        }
        Ok(map)
    }

    /// 增量更新文件快照：与旧快照比对后仅写入差异条目。
    /// 大目录下避免整表 DELETE + 重灌（初始同步旧表为空时直接走整表替换）。
    pub fn sync_file_state(
        &self,
        pair_id: &str,
        new: &BTreeMap<String, FileMeta>,
    ) -> Result<(), String> {
        let old = self.load_file_state(pair_id)?;
        if old.is_empty() {
            return self.replace_file_state(pair_id, new);
        }

        let fail = |e: rusqlite::Error| {
            crate::msg!("更新文件快照失败: {e}", "Failed to update file snapshot: {e}")
        };
        let mut removals: Vec<&str> = Vec::new();
        for (k, ov) in &old {
            match new.get(k) {
                None => removals.push(&ov.rel),
                // rel 大小写/分隔符变化：规范化 key 相同但主键不同，旧行需删
                Some(nv) if nv.rel != ov.rel => removals.push(&ov.rel),
                _ => {}
            }
        }
        let mut upserts: Vec<&FileMeta> = Vec::new();
        for (k, nv) in new {
            match old.get(k) {
                Some(ov)
                    if ov.rel == nv.rel && ov.size == nv.size && ov.mtime_ns == nv.mtime_ns => {}
                _ => upserts.push(nv),
            }
        }
        if removals.is_empty() && upserts.is_empty() {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction().map_err(fail)?;
        {
            let mut st = tx
                .prepare("DELETE FROM file_state WHERE pair_id = ?1 AND rel_path = ?2")
                .map_err(fail)?;
            for rel in &removals {
                st.execute(params![pair_id, rel]).map_err(fail)?;
            }
        }
        {
            let mut st = tx
                .prepare(
                    "INSERT OR REPLACE INTO file_state (pair_id, rel_path, size, mtime_ns)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(fail)?;
            for m in &upserts {
                st.execute(params![pair_id, m.rel, m.size as i64, m.mtime_ns])
                    .map_err(fail)?;
            }
        }
        tx.commit().map_err(fail)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_state_roundtrip_and_normalize() {
        let dir = std::env::temp_dir().join(format!("vinesync_db_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("t.db")).unwrap();

        let mut m = BTreeMap::new();
        m.insert(
            "a/b.txt".to_string(),
            FileMeta { rel: "A\\B.txt".into(), size: 10, mtime_ns: 123 },
        );
        m.insert(
            "c.txt".to_string(),
            FileMeta { rel: "c.txt".into(), size: 20, mtime_ns: 456 },
        );
        db.replace_file_state("p1", &m).unwrap();
        db.replace_file_state("p2", &BTreeMap::new()).unwrap();

        let loaded = db.load_file_state("p1").unwrap();
        assert_eq!(loaded.len(), 2);
        // key 为规范化路径（小写、分隔符统一），rel 保留原始写法
        let f = loaded.get("a/b.txt").expect("normalized key");
        assert_eq!(f.rel, "A\\B.txt");
        assert_eq!(f.size, 10);
        assert_eq!(f.mtime_ns, 123);

        // 其他 pair 不受影响；空/缺失 pair 返回空表
        assert!(db.load_file_state("p2").unwrap().is_empty());
        assert!(db.load_file_state("missing").unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_state_delta_writes_only_differences() {
        let dir = std::env::temp_dir().join(format!("vinesync_db_delta_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("t.db")).unwrap();

        // 初始整表写入
        let mut base = BTreeMap::new();
        base.insert("a.txt".into(), FileMeta { rel: "a.txt".into(), size: 10, mtime_ns: 1 });
        base.insert("b.txt".into(), FileMeta { rel: "b.txt".into(), size: 20, mtime_ns: 2 });
        db.sync_file_state("p", &base).unwrap();
        assert_eq!(db.file_state_count("p").unwrap(), 2);

        // 增量：a 变化、b 删除、c 新增
        let mut next = BTreeMap::new();
        next.insert("a.txt".into(), FileMeta { rel: "a.txt".into(), size: 11, mtime_ns: 3 });
        next.insert("c.txt".into(), FileMeta { rel: "c.txt".into(), size: 30, mtime_ns: 4 });
        db.sync_file_state("p", &next).unwrap();
        let loaded = db.load_file_state("p").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["a.txt"].size, 11);
        assert!(loaded.contains_key("c.txt"));
        assert!(!loaded.contains_key("b.txt"));

        // 幂等：相同内容再次同步不报错、结果不变
        db.sync_file_state("p", &next).unwrap();
        assert_eq!(db.file_state_count("p").unwrap(), 2);

        // rel 大小写变化（规范化 key 相同）：旧行删除、新行插入，不产生重复
        let mut cased = BTreeMap::new();
        cased.insert("c.txt".into(), FileMeta { rel: "C.txt".into(), size: 30, mtime_ns: 4 });
        cased.insert("a.txt".into(), FileMeta { rel: "a.txt".into(), size: 11, mtime_ns: 3 });
        db.sync_file_state("p", &cased).unwrap();
        let loaded = db.load_file_state("p").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["c.txt"].rel, "C.txt");
        assert_eq!(db.file_state_count("p").unwrap(), 2, "大小写变化不应产生重复行");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
