use rusqlite::{params, Connection};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_core",
        sql: include_str!("../../migrations/0001_core.sql"),
    },
    Migration {
        version: 2,
        name: "0002_type_ext",
        sql: include_str!("../../migrations/0002_type_ext.sql"),
    },
    Migration {
        version: 3,
        name: "0003_fts",
        sql: include_str!("../../migrations/0003_fts.sql"),
    },
    Migration {
        version: 4,
        name: "0004_collab",
        sql: include_str!("../../migrations/0004_collab.sql"),
    },
    Migration {
        version: 5,
        name: "0005_meta_state",
        sql: include_str!("../../migrations/0005_meta_state.sql"),
    },
    Migration {
        version: 6,
        name: "0006_dupes",
        sql: include_str!("../../migrations/0006_dupes.sql"),
    },
    Migration {
        version: 7,
        name: "0007_ocr",
        sql: include_str!("../../migrations/0007_ocr.sql"),
    },
    Migration {
        version: 8,
        name: "0008_item_category_source",
        sql: include_str!("../../migrations/0008_item_category_source.sql"),
    },
];

pub fn applied_versions(conn: &Connection) -> Result<Vec<i64>, rusqlite::Error> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    rows.collect()
}

pub fn migrate(conn: &mut Connection) -> Result<Vec<i64>, rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    let applied = applied_versions(conn)?;
    let mut newly: Vec<i64> = Vec::new();
    for m in MIGRATIONS {
        if applied.contains(&m.version) {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)",
            params![m.version, m.name],
        )?;
        tx.commit()?;
        newly.push(m.version);
    }
    Ok(newly)
}
