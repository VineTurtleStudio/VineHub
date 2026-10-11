use crate::meta::{abs_path_map, MetaOutcome, MetaProgress, MetaStats};
use rusqlite::OpenFlags;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// beets 只读打开 library.db（file:...?mode=ro，绝不写对方库）。
/// 按绝对路径匹配本库 music 条目，回填 music_tracks + meta_state='done'。
pub fn import(
    conn: &mut rusqlite::Connection,
    beets_db: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&MetaProgress),
) -> Result<MetaOutcome, String> {
    if !beets_db.is_file() {
        return Err(format!("beets 库不存在: {}", beets_db.display()));
    }
    let uri = format!(
        "file:{}?mode=ro",
        beets_db.to_string_lossy().replace('\\', "/")
    );
    let remote = rusqlite::Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开 beets 库失败: {e}"))?;

    let rows = load_beets_rows(&remote)?;
    let map = abs_path_map(conn, "music")?;
    let total = rows.len();
    let mut stats = MetaStats::default();

    progress(&MetaProgress {
        phase: "import".into(),
        index: 0,
        total,
        rel_path: String::new(),
        status: "running".into(),
        message: format!("beets 待匹配 {total} 条"),
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
            apply_beets_row(conn, item_id, row)?;
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
        "beets 导入完成 · 匹配 {} · 未命中 {}",
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

pub struct BeetsRow {
    pub path: String,
    pub artist: Option<String>,
    pub albumartist: Option<String>,
    pub album: Option<String>,
    pub disc: Option<i64>,
    pub track: Option<i64>,
    pub duration: Option<f64>,
    pub format: Option<String>,
    pub bitrate: Option<i64>,
    pub mbid: Option<String>,
    pub year: Option<i64>,
}

fn opt_str(v: rusqlite::Result<rusqlite::types::ValueRef<'_>>) -> Option<String> {
    match v.ok()? {
        rusqlite::types::ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        }
        rusqlite::types::ValueRef::Blob(b) => {
            let s = String::from_utf8_lossy(b).into_owned();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        }
        rusqlite::types::ValueRef::Null => None,
        rusqlite::types::ValueRef::Integer(i) => Some(i.to_string()),
        rusqlite::types::ValueRef::Real(f) => Some(f.to_string()),
    }
}

fn load_beets_rows(db: &rusqlite::Connection) -> Result<Vec<BeetsRow>, String> {
    let mut stmt = db
        .prepare(
            "SELECT path, artist, albumartist, album, disc, track, length, format, bitrate,
                    mb_trackid, year FROM items",
        )
        .map_err(|e| format!("beets items 表结构不符: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            let path = match r.get_ref(0)? {
                rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                rusqlite::types::ValueRef::Blob(b) => String::from_utf8_lossy(b).into_owned(),
                _ => String::new(),
            };
            Ok(BeetsRow {
                path,
                artist: opt_str(r.get_ref(1)),
                albumartist: opt_str(r.get_ref(2)),
                album: opt_str(r.get_ref(3)),
                disc: r.get::<_, Option<i64>>(4).ok().flatten(),
                track: r.get::<_, Option<i64>>(5).ok().flatten(),
                duration: r.get::<_, Option<f64>>(6).ok().flatten(),
                format: opt_str(r.get_ref(7)),
                bitrate: r.get::<_, Option<i64>>(8).ok().flatten(),
                mbid: opt_str(r.get_ref(9)),
                year: r.get::<_, Option<i64>>(10).ok().flatten(),
            })
        })
        .map_err(|e| format!("读取 beets items 失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let row = row.map_err(|e| format!("读取 beets 行失败: {e}"))?;
        if !row.path.is_empty() {
            out.push(row);
        }
    }
    Ok(out)
}

fn apply_beets_row(
    conn: &rusqlite::Connection,
    item_id: i64,
    row: &BeetsRow,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO music_tracks (item_id, artist, albumartist, album, disc, track, duration, format, bitrate, mbid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(item_id) DO UPDATE SET
           artist = COALESCE(excluded.artist, music_tracks.artist),
           albumartist = COALESCE(excluded.albumartist, music_tracks.albumartist),
           album = COALESCE(excluded.album, music_tracks.album),
           disc = COALESCE(excluded.disc, music_tracks.disc),
           track = COALESCE(excluded.track, music_tracks.track),
           duration = COALESCE(excluded.duration, music_tracks.duration),
           format = COALESCE(excluded.format, music_tracks.format),
           bitrate = COALESCE(excluded.bitrate, music_tracks.bitrate),
           mbid = COALESCE(excluded.mbid, music_tracks.mbid)",
        rusqlite::params![
            item_id,
            row.artist,
            row.albumartist,
            row.album,
            row.disc,
            row.track,
            row.duration,
            row.format,
            row.bitrate,
            row.mbid
        ],
    )
    .map_err(|e| format!("写入 music_tracks 失败 id={item_id}: {e}"))?;
    conn.execute(
        "UPDATE items SET meta_state = 'done', year = COALESCE(year, ?1),
         updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![row.year, item_id],
    )
    .map_err(|e| format!("写入 meta_state 失败 id={item_id}: {e}"))?;
    crate::fts::sync_item(conn, item_id)?;
    Ok(())
}

/// 供测试与复用：直接用行数据匹配（不打开远程库）
pub fn match_rows(conn: &mut rusqlite::Connection, rows: &[BeetsRow]) -> Result<MetaStats, String> {
    let map: HashMap<String, i64> = abs_path_map(conn, "music")?;
    let mut stats = MetaStats::default();
    for row in rows {
        let key = crate::meta::normalize_key(&row.path);
        if let Some(&item_id) = map.get(&key) {
            apply_beets_row(conn, item_id, row)?;
            stats.imported += 1;
        } else {
            stats.skipped += 1;
        }
    }
    Ok(stats)
}
