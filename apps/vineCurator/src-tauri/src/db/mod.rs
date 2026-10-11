pub mod migrate;
pub mod repo;

use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;
pub type DbError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub struct DbState {
    pub pool: Pool,
    pub db_path: std::path::PathBuf,
}

pub fn open_pool(path: &Path) -> Result<Pool, DbError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let manager = SqliteConnectionManager::file(path).with_init(|conn| {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
    });
    let pool = r2d2::Pool::builder().max_size(8).build(manager)?;
    Ok(pool)
}
