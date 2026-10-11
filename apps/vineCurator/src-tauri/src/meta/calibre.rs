use crate::meta::{abs_path_map, MetaOutcome, MetaProgress, MetaStats};
use rusqlite::OpenFlags;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Calibre 只读打开 <library>/metadata.db（mode=ro）。
/// Calibre 文件路径 = library_dir/books.path/data.name.<format 小写>。
/// 按绝对路径匹配本库 book 条目，回填 book_items + meta_state='done'。
pub fn import(
    conn: &mut rusqlite::Connection,
    library_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&MetaProgress),
) -> Result<MetaOutcome, String> {
    let db_path = if library_dir.is_dir() {
        library_dir.join("metadata.db")
    } else {
        library_dir.to_path_buf()
    };
    if !db_path.is_file() {
        return Err(format!("Calibre metadata.db 不存在: {}", db_path.display()));
    }
    let root_dir = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| library_dir.to_path_buf());

    let uri = format!(
        "file:{}?mode=ro",
        db_path.to_string_lossy().replace('\\', "/")
    );
    let remote = rusqlite::Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开 Calibre 库失败: {e}"))?;

    let rows = load_calibre_rows(&remote, &root_dir)?;
    let map = abs_path_map(conn, "book")?;
    let total = rows.len();
    let mut stats = MetaStats::default();

    progress(&MetaProgress {
        phase: "import".into(),
        index: 0,
        total,
        rel_path: String::new(),
        status: "running".into(),
        message: format!("Calibre 待匹配 {total} 条"),
        stats: stats.clone(),
    });

    for (i, row) in rows.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            progress(&MetaProgress {
                phase: "finished".into(),
                index: i,
                total,
                rel_path: String::new(),
                status: "cancelled".into(),
                message: format!("导入已取消 · 匹配 {}", stats.imported),
                stats: stats.clone(),
            });
            return Ok(MetaOutcome {
                stats,
                status: "cancelled".into(),
            });
        }
        let key = crate::meta::normalize_key(&row.path);
        if let Some(&item_id) = map.get(&key) {
            apply_calibre_row(conn, item_id, row)?;
            stats.imported += 1;
        } else {
            stats.skipped += 1;
        }
        if (i + 1) % 10 == 0 || i + 1 == total {
            progress(&MetaProgress {
                phase: "import".into(),
                index: i + 1,
                total,
                rel_path: String::new(),
                status: "running".into(),
                message: String::new(),
                stats: stats.clone(),
            });
        }
    }

    let message = format!(
        "Calibre 导入完成 · 匹配 {} · 未命中 {}",
        stats.imported, stats.skipped
    );
    progress(&MetaProgress {
        phase: "finished".into(),
        index: total,
        total,
        rel_path: String::new(),
        status: "done".into(),
        message,
        stats: stats.clone(),
    });
    Ok(MetaOutcome {
        stats,
        status: "done".into(),
    })
}

pub struct CalibreRow {
    pub path: String,
    pub author: Option<String>,
    pub series: Option<String>,
    pub volume: Option<String>,
    pub isbn: Option<String>,
    pub publisher: Option<String>,
    pub year: Option<i64>,
    pub format: Option<String>,
    rel_dir: Option<String>,
    data_name: Option<String>,
}

fn empty_to_none(s: Option<String>) -> Option<String> {
    s.filter(|x| !x.trim().is_empty())
}

fn load_calibre_rows(
    db: &rusqlite::Connection,
    root_dir: &Path,
) -> Result<Vec<CalibreRow>, String> {
    let mut stmt = db
        .prepare(
            "SELECT b.id, d.format, d.name, b.path, b.series_index,
                    (SELECT group_concat(a.name, ', ') FROM books_authors_link l
                     JOIN authors a ON a.id = l.author WHERE l.book = b.id) AS authors,
                    (SELECT s.name FROM books_series_link l
                     JOIN series s ON s.id = l.series WHERE l.book = b.id) AS series,
                    (SELECT group_concat(p.name, ', ') FROM books_publishers_link l
                     JOIN publishers p ON p.id = l.publisher WHERE l.book = b.id) AS publishers,
                    (SELECT val FROM identifiers i WHERE i.book = b.id AND lower(i.type) = 'isbn') AS isbn
             FROM books b JOIN data d ON d.book = b.id",
        )
        .map_err(|e| format!("Calibre metadata.db 结构不符: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            let _book_id: i64 = r.get(0)?;
            let format: Option<String> = r.get(1)?;
            let name: Option<String> = r.get(2)?;
            let path: Option<String> = r.get(3)?;
            let series_index: Option<f64> = r.get(4)?;
            let authors: Option<String> = r.get(5)?;
            let series: Option<String> = r.get(6)?;
            let publishers: Option<String> = r.get(7)?;
            let isbn: Option<String> = r.get(8)?;
            Ok(CalibreRow {
                path: String::new(),
                author: empty_to_none(authors),
                series: empty_to_none(series),
                volume: series_index.filter(|v| *v > 0.0).map(format_index),
                isbn: empty_to_none(isbn),
                publisher: empty_to_none(publishers),
                year: None,
                format: format.map(|f| f.to_lowercase()),
                rel_dir: path,
                data_name: name,
            })
        })
        .map_err(|e| format!("读取 Calibre books 失败: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        let mut row = row.map_err(|e| format!("读取 Calibre 行失败: {e}"))?;
        let (Some(rel_dir), Some(name), Some(format)) =
            (row.rel_dir.take(), row.data_name.take(), row.format.clone())
        else {
            continue;
        };
        let file = format!("{name}.{format}");
        let abs = root_dir.join(&rel_dir).join(&file);
        row.path = abs.to_string_lossy().into_owned();
        out.push(row);
    }
    Ok(out)
}

/// series_index REAL → "1" / "1.5"
pub fn format_index(idx: f64) -> String {
    if idx.fract().abs() < 1e-9 {
        format!("{}", idx as i64)
    } else {
        format!("{idx}")
    }
}

fn apply_calibre_row(
    conn: &rusqlite::Connection,
    item_id: i64,
    row: &CalibreRow,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO book_items (item_id, author, series, volume, isbn, publisher, format)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(item_id) DO UPDATE SET
           author = COALESCE(excluded.author, book_items.author),
           series = COALESCE(excluded.series, book_items.series),
           volume = COALESCE(excluded.volume, book_items.volume),
           isbn = COALESCE(excluded.isbn, book_items.isbn),
           publisher = COALESCE(excluded.publisher, book_items.publisher),
           format = COALESCE(excluded.format, book_items.format)",
        rusqlite::params![
            item_id,
            row.author,
            row.series,
            row.volume,
            row.isbn,
            row.publisher,
            row.format
        ],
    )
    .map_err(|e| format!("写入 book_items 失败 id={item_id}: {e}"))?;
    conn.execute(
        "UPDATE items SET meta_state = 'done', year = COALESCE(year, ?1),
         updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![row.year, item_id],
    )
    .map_err(|e| format!("写入 meta_state 失败 id={item_id}: {e}"))?;
    crate::fts::sync_item(conn, item_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_index_variants() {
        assert_eq!(format_index(1.0), "1");
        assert_eq!(format_index(0.0), "0");
        assert_eq!(format_index(2.5), "2.5");
    }
}
