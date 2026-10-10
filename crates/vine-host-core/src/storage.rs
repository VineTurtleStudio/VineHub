//! 宿主配置存储：单一 SQLite（`plugins` / `settings` / `tasks` 三表）。
//!
//! 各插件业务库原地不动（Curator FTS5、Sync 历史表等），hub 只登记路径。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Row};

/// 存储错误。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("SQLite 错误: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("数据损坏: {0}")]
    Corrupt(String),
}

/// 插件登记记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    /// "external" | "sidecar"
    pub kind: String,
    pub enabled: bool,
    /// 设置页 exe 路径覆盖（None = 按 manifest 解析）。
    pub entry_override: Option<String>,
    pub data_dir: String,
    /// Unix 秒。
    pub installed_at: i64,
}

/// 任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// 任务记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub plugin_id: String,
    pub method: String,
    pub status: TaskStatus,
    pub created_at: i64,
    pub updated_at: i64,
    /// 附加信息（JSON 字符串，如结果摘要/错误）。
    pub detail: Option<String>,
}

/// hub 单一 SQLite 存储。
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开（必要时创建）数据库文件并执行迁移。
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StorageError> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// 版本化迁移（`PRAGMA user_version`）。
    fn migrate(&self) -> Result<(), StorageError> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS plugins (
                    id             TEXT PRIMARY KEY,
                    name           TEXT NOT NULL,
                    version        TEXT NOT NULL,
    api_version    TEXT NOT NULL,
                    kind           TEXT NOT NULL,
                    enabled        INTEGER NOT NULL DEFAULT 1,
                    entry_override TEXT,
                    data_dir       TEXT NOT NULL,
                    installed_at   INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS settings (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS tasks (
                    id         TEXT PRIMARY KEY,
                    plugin_id  TEXT NOT NULL,
                    method     TEXT NOT NULL,
                    status     TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    detail     TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_tasks_plugin ON tasks(plugin_id);
                "#,
            )?;
            self.conn
                .pragma_update(None, "user_version", 1)?;
        }
        Ok(())
    }

    // ---------- plugins ----------

    /// 登记（存在则更新可变字段）。
    pub fn upsert_plugin(&self, record: &PluginRecord) -> Result<(), StorageError> {
        self.conn.execute(
            r#"
            INSERT INTO plugins (id, name, version, api_version, kind, enabled, entry_override, data_dir, installed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                api_version = excluded.api_version,
                kind = excluded.kind,
                entry_override = excluded.entry_override,
                data_dir = excluded.data_dir
            "#,
            params![
                record.id,
                record.name,
                record.version,
                record.api_version,
                record.kind,
                record.enabled as i64,
                record.entry_override,
                record.data_dir,
                record.installed_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_plugin(&self, id: &str) -> Result<Option<PluginRecord>, StorageError> {
        let record = self
            .conn
            .query_row(
                "SELECT id, name, version, api_version, kind, enabled, entry_override, data_dir, installed_at
                 FROM plugins WHERE id = ?1",
                params![id],
                Self::row_to_plugin,
            )
            .optional()?;
        Ok(record)
    }

    pub fn list_plugins(&self) -> Result<Vec<PluginRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, version, api_version, kind, enabled, entry_override, data_dir, installed_at
             FROM plugins ORDER BY id",
        )?;
        let rows = stmt.query_map([], Self::row_to_plugin)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_plugin_enabled(&self, id: &str, enabled: bool) -> Result<bool, StorageError> {
        let changed = self.conn.execute(
            "UPDATE plugins SET enabled = ?2 WHERE id = ?1",
            params![id, enabled as i64],
        )?;
        Ok(changed > 0)
    }

    /// 设置 / 清除 exe 路径覆盖。
    pub fn set_entry_override(&self, id: &str, path: Option<&str>) -> Result<bool, StorageError> {
        let changed = self.conn.execute(
            "UPDATE plugins SET entry_override = ?2 WHERE id = ?1",
            params![id, path],
        )?;
        Ok(changed > 0)
    }

    pub fn remove_plugin(&self, id: &str) -> Result<bool, StorageError> {
        let changed = self
            .conn
            .execute("DELETE FROM plugins WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    fn row_to_plugin(row: &Row<'_>) -> rusqlite::Result<PluginRecord> {
        Ok(PluginRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            api_version: row.get(3)?,
            kind: row.get(4)?,
            enabled: row.get::<_, i64>(5)? != 0,
            entry_override: row.get(6)?,
            data_dir: row.get(7)?,
            installed_at: row.get(8)?,
        })
    }

    // ---------- settings ----------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<bool, StorageError> {
        let changed = self
            .conn
            .execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(changed > 0)
    }

    // ---------- tasks ----------

    pub fn insert_task(&self, task: &TaskRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO tasks (id, plugin_id, method, status, created_at, updated_at, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                task.id,
                task.plugin_id,
                task.method,
                task.status.as_str(),
                task.created_at,
                task.updated_at,
                task.detail,
            ],
        )?;
        Ok(())
    }

    pub fn update_task(
        &self,
        id: &str,
        status: TaskStatus,
        updated_at: i64,
        detail: Option<&str>,
    ) -> Result<bool, StorageError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET status = ?2, updated_at = ?3, detail = ?4 WHERE id = ?1",
            params![id, status.as_str(), updated_at, detail],
        )?;
        Ok(changed > 0)
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskRecord>, StorageError> {
        let task = self
            .conn
            .query_row(
                "SELECT id, plugin_id, method, status, created_at, updated_at, detail
                 FROM tasks WHERE id = ?1",
                params![id],
                Self::row_to_task,
            )
            .optional()?;
        Ok(task)
    }

    pub fn list_tasks(&self, plugin_id: Option<&str>) -> Result<Vec<TaskRecord>, StorageError> {
        let mut tasks = match plugin_id {
            Some(plugin_id) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, plugin_id, method, status, created_at, updated_at, detail
                     FROM tasks WHERE plugin_id = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![plugin_id], Self::row_to_task)?;
                rows.collect::<Result<Vec<_>, _>>()?
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, plugin_id, method, status, created_at, updated_at, detail
                     FROM tasks ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([], Self::row_to_task)?;
                rows.collect::<Result<Vec<_>, _>>()?
            }
        };
        tasks.shrink_to_fit();
        Ok(tasks)
    }

    fn row_to_task(row: &Row<'_>) -> rusqlite::Result<TaskRecord> {
        let status_str: String = row.get(3)?;
        let status = TaskStatus::parse(&status_str)
            .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(std::io::Error::other(format!("未知任务状态: {status_str}")))))?;
        Ok(TaskRecord {
            id: row.get(0)?,
            plugin_id: row.get(1)?,
            method: row.get(2)?,
            status,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
            detail: row.get(6)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plugin(id: &str) -> PluginRecord {
        PluginRecord {
            id: id.to_string(),
            name: format!("Plugin {id}"),
            version: "0.1.0".into(),
            api_version: "1.0".into(),
            kind: "external".into(),
            enabled: true,
            entry_override: None,
            data_dir: id.to_string(),
            installed_at: 1_700_000_000,
        }
    }

    #[test]
    fn migration_creates_three_tables_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("hub.db");

        {
            let store = Store::open(&db_path).unwrap();
            let names: Vec<String> = {
                let mut stmt = store
                    .conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                    .unwrap();
                let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
                rows.collect::<Result<Vec<_>, _>>().unwrap()
            };
            assert!(names.contains(&"plugins".to_string()));
            assert!(names.contains(&"settings".to_string()));
            assert!(names.contains(&"tasks".to_string()));
        }

        // 重复打开（迁移幂等）不报错且数据保留
        let store = Store::open(&db_path).unwrap();
        assert!(store.list_plugins().unwrap().is_empty());
        store.set_setting("k", "v").unwrap();
        drop(store);
        let store = Store::open(&db_path).unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn plugin_crud_and_entry_override() {
        let store = Store::open_in_memory().unwrap();
        let mut record = sample_plugin("vine.pulse");
        store.upsert_plugin(&record).unwrap();

        let loaded = store.get_plugin("vine.pulse").unwrap().unwrap();
        assert_eq!(loaded, record);

        // 路径覆盖项（Windows 路径）
        let override_path = "D:\\tools\\vine-pulse.exe";
        assert!(store.set_entry_override("vine.pulse", Some(override_path)).unwrap());
        let loaded = store.get_plugin("vine.pulse").unwrap().unwrap();
        assert_eq!(loaded.entry_override.as_deref(), Some(override_path));

        // 清除覆盖
        assert!(store.set_entry_override("vine.pulse", None).unwrap());
        assert!(store.get_plugin("vine.pulse").unwrap().unwrap().entry_override.is_none());

        // 禁用 / 启用
        assert!(store.set_plugin_enabled("vine.pulse", false).unwrap());
        assert!(!store.get_plugin("vine.pulse").unwrap().unwrap().enabled);

        // upsert 更新版本、保留 enabled 语义（enabled 不在 conflict 更新列）
        record.version = "0.2.0".into();
        store.upsert_plugin(&record).unwrap();
        let loaded = store.get_plugin("vine.pulse").unwrap().unwrap();
        assert_eq!(loaded.version, "0.2.0");
        assert!(!loaded.enabled, "upsert 不应重置 enabled");

        // 不存在的 id 更新返回 false
        assert!(!store.set_entry_override("vine.none", Some("x")).unwrap());

        // 删除
        assert!(store.remove_plugin("vine.pulse").unwrap());
        assert!(store.get_plugin("vine.pulse").unwrap().is_none());
        assert!(!store.remove_plugin("vine.pulse").unwrap());
    }

    #[test]
    fn settings_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.get_setting("plugins.dir").unwrap().is_none());
        store.set_setting("plugins.dir", "D:/VineHub/plugins").unwrap();
        assert_eq!(
            store.get_setting("plugins.dir").unwrap().as_deref(),
            Some("D:/VineHub/plugins")
        );
        store.set_setting("plugins.dir", "E:/other").unwrap();
        assert_eq!(
            store.get_setting("plugins.dir").unwrap().as_deref(),
            Some("E:/other")
        );
        assert!(store.delete_setting("plugins.dir").unwrap());
        assert!(store.get_setting("plugins.dir").unwrap().is_none());
    }

    #[test]
    fn task_lifecycle_and_listing() {
        let store = Store::open_in_memory().unwrap();
        let task = TaskRecord {
            id: "t-1".into(),
            plugin_id: "vine.pulse".into(),
            method: "scan.start".into(),
            status: TaskStatus::Pending,
            created_at: 100,
            updated_at: 100,
            detail: None,
        };
        store.insert_task(&task).unwrap();
        assert!(store.update_task("t-1", TaskStatus::Running, 200, None).unwrap());
        assert!(
            store
                .update_task("t-1", TaskStatus::Done, 300, Some(r#"{"ok":5,"bad":1}"#))
                .unwrap()
        );

        let loaded = store.get_task("t-1").unwrap().unwrap();
        assert_eq!(loaded.status, TaskStatus::Done);
        assert_eq!(loaded.updated_at, 300);
        assert_eq!(loaded.detail.as_deref(), Some(r#"{"ok":5,"bad":1}"#));

        // 按插件过滤
        store
            .insert_task(&TaskRecord {
                id: "t-2".into(),
                plugin_id: "vine.sync".into(),
                method: "task.run".into(),
                status: TaskStatus::Pending,
                created_at: 150,
                updated_at: 150,
                detail: None,
            })
            .unwrap();
        assert_eq!(store.list_tasks(None).unwrap().len(), 2);
        let pulse_tasks = store.list_tasks(Some("vine.pulse")).unwrap();
        assert_eq!(pulse_tasks.len(), 1);
        assert_eq!(pulse_tasks[0].id, "t-1");

        assert!(!store.update_task("t-none", TaskStatus::Failed, 1, None).unwrap());
    }
}
