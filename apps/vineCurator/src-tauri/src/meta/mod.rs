use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod beets;
pub mod calibre;
pub mod exiftool;
pub mod ffprobe;
pub mod runner;

use runner::Runner;

const PROGRESS_EVERY: usize = 10;

#[derive(Debug, Default, Clone, Serialize)]
pub struct MetaStats {
    pub probed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub imported: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetaProgress {
    /// probe | import | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rel_path: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub stats: MetaStats,
}

impl MetaProgress {
    pub fn failure(message: impl Into<String>) -> Self {
        MetaProgress {
            phase: "finished".into(),
            index: 0,
            total: 0,
            rel_path: String::new(),
            status: "error".into(),
            message: message.into(),
            stats: MetaStats::default(),
        }
    }
}

#[derive(Debug)]
pub struct MetaOutcome {
    pub stats: MetaStats,
    /// done | cancelled
    pub status: String,
}

pub struct PendingItem {
    pub id: i64,
    pub rel_path: String,
    pub type_name: String,
    pub root_path: String,
}

/// 待探测条目：四类媒体 + meta_state='none' + 未缺失（可选限定根目录）
pub fn pending_probe(conn: &Connection, root_id: Option<i64>) -> Result<Vec<PendingItem>, String> {
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.rel_path, i.type, r.path FROM items i
             JOIN roots r ON r.id = i.root_id
             WHERE i.meta_state = 'none' AND i.missing = 0
               AND i.type IN ('music','video','book','comic')
               AND (?1 IS NULL OR i.root_id = ?1)
             ORDER BY i.id",
        )
        .map_err(|e| format!("查询待探测条目失败: {e}"))?;
    let rows = stmt
        .query_map([root_id], |r| {
            Ok(PendingItem {
                id: r.get(0)?,
                rel_path: r.get(1)?,
                type_name: r.get(2)?,
                root_path: r.get(3)?,
            })
        })
        .map_err(|e| format!("查询待探测条目失败: {e}"))?;
    for row in rows {
        out.push(row.map_err(|e| format!("读取行失败: {e}"))?);
    }
    Ok(out)
}

fn set_meta_state(
    conn: &Connection,
    item_id: i64,
    state: &str,
    json: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE items SET meta_state = ?1, metadata = COALESCE(?2, metadata),
         updated_at = datetime('now') WHERE id = ?3",
        params![state, json, item_id],
    )
    .map_err(|e| format!("写入 meta_state 失败 id={item_id}: {e}"))?;
    Ok(())
}

fn update_year(conn: &Connection, item_id: i64, year: Option<i64>) -> Result<(), String> {
    if let Some(y) = year {
        conn.execute(
            "UPDATE items SET year = COALESCE(year, ?1), updated_at = datetime('now') WHERE id = ?2",
            params![y, item_id],
        )
        .map_err(|e| format!("写入 year 失败 id={item_id}: {e}"))?;
    }
    Ok(())
}

/// 探测一个根目录（或全部）的 pending 条目：Runner 探测 → JSON 落 items.metadata + 扩展表。
/// 已探测（meta_state != 'none'）跳过；文件缺失跳过并保持 none；取消不改状态。
pub fn probe_root(
    conn: &mut Connection,
    root_id: Option<i64>,
    runner: &mut dyn Runner,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&MetaProgress),
) -> Result<MetaOutcome, String> {
    let pending = pending_probe(conn, root_id)?;
    let total = pending.len();
    let mut stats = MetaStats::default();

    progress(&MetaProgress {
        phase: "probe".into(),
        index: 0,
        total,
        rel_path: String::new(),
        status: "running".into(),
        message: format!("待探测 {total} 条"),
        stats: stats.clone(),
    });

    for (i, item) in pending.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            progress(&MetaProgress {
                phase: "finished".into(),
                index: i,
                total,
                rel_path: item.rel_path.clone(),
                status: "cancelled".into(),
                message: format!("探测已取消 · 完成 {} · 失败 {}", stats.probed, stats.failed),
                stats: stats.clone(),
            });
            return Ok(MetaOutcome {
                stats,
                status: "cancelled".into(),
            });
        }

        let abs = PathBuf::from(&item.root_path).join(&item.rel_path);
        if !abs.exists() {
            stats.skipped += 1;
            continue;
        }

        match runner.probe(&abs, &item.type_name) {
            Ok(json) => {
                if let Err(e) = apply_probe_json(conn, item.id, &item.type_name, &json) {
                    stats.failed += 1;
                    set_meta_state(conn, item.id, "failed", None)?;
                    progress(&MetaProgress {
                        phase: "probe".into(),
                        index: i + 1,
                        total,
                        rel_path: item.rel_path.clone(),
                        status: "error".into(),
                        message: format!("{}: {e}", item.rel_path),
                        stats: stats.clone(),
                    });
                    continue;
                }
                stats.probed += 1;
            }
            Err(e) => {
                stats.failed += 1;
                set_meta_state(conn, item.id, "failed", None)?;
                progress(&MetaProgress {
                    phase: "probe".into(),
                    index: i + 1,
                    total,
                    rel_path: item.rel_path.clone(),
                    status: "error".into(),
                    message: format!("{}: {e}", item.rel_path),
                    stats: stats.clone(),
                });
                continue;
            }
        }

        if (i + 1) % PROGRESS_EVERY == 0 || i + 1 == total {
            progress(&MetaProgress {
                phase: "probe".into(),
                index: i + 1,
                total,
                rel_path: item.rel_path.clone(),
                status: "running".into(),
                message: String::new(),
                stats: stats.clone(),
            });
        }
    }

    let message = format!(
        "探测完成 · 成功 {} · 跳过 {} · 失败 {}",
        stats.probed, stats.skipped, stats.failed
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

/// 解析探测 JSON 并写扩展表（含 meta_state='done' + items.year）
pub fn apply_probe_json(
    conn: &Connection,
    item_id: i64,
    type_name: &str,
    json: &str,
) -> Result<(), String> {
    let table = crate::scan::classify::ext_table(type_name)
        .ok_or_else(|| format!("类型无扩展表: {type_name}"))?;
    let year;
    match table {
        "video_items" => {
            let d = ffprobe::parse(json)?;
            year = d.year();
            apply_video(conn, item_id, &d)?;
        }
        "music_tracks" => {
            let d = ffprobe::parse(json)?;
            year = d.year();
            apply_music(conn, item_id, &d)?;
        }
        "book_items" => {
            let d = exiftool::parse(json)?;
            year = d.year();
            apply_book(conn, item_id, &d)?;
        }
        "comic_items" => {
            let d = exiftool::parse(json)?;
            year = d.year();
            apply_comic(conn, item_id, &d)?;
        }
        other => return Err(format!("非法扩展表: {other}")),
    }
    set_meta_state(conn, item_id, "done", Some(json))?;
    update_year(conn, item_id, year)?;
    crate::fts::sync_item(conn, item_id)?;
    Ok(())
}

fn apply_video(conn: &Connection, item_id: i64, d: &ffprobe::FfprobeData) -> Result<(), String> {
    let resolution = d.resolution();
    conn.execute(
        "INSERT INTO video_items (item_id, resolution, codec, duration)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(item_id) DO UPDATE SET
           resolution = COALESCE(excluded.resolution, video_items.resolution),
           codec = COALESCE(excluded.codec, video_items.codec),
           duration = COALESCE(excluded.duration, video_items.duration)",
        params![item_id, resolution, d.video_codec(), d.duration()],
    )
    .map_err(|e| format!("写入 video_items 失败 id={item_id}: {e}"))?;
    Ok(())
}

fn apply_music(conn: &Connection, item_id: i64, d: &ffprobe::FfprobeData) -> Result<(), String> {
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
        params![
            item_id,
            d.tag(&["artist", "album_artist"]),
            d.tag(&["album_artist", "albumartist"]),
            d.tag(&["album"]),
            d.tag_int(&["disc", "part"]),
            d.tag_int(&["track", "tracknumber"]),
            d.duration(),
            d.format.format_name,
            d.bit_rate(),
            d.tag(&["musicbrainz_trackid", "musicbrainz_track_id"])
        ],
    )
    .map_err(|e| format!("写入 music_tracks 失败 id={item_id}: {e}"))?;
    Ok(())
}

fn apply_book(conn: &Connection, item_id: i64, d: &exiftool::ExifData) -> Result<(), String> {
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
        params![
            item_id,
            d.author(),
            d.series(),
            d.volume(),
            d.isbn(),
            d.publisher(),
            d.format()
        ],
    )
    .map_err(|e| format!("写入 book_items 失败 id={item_id}: {e}"))?;
    Ok(())
}

fn apply_comic(conn: &Connection, item_id: i64, d: &exiftool::ExifData) -> Result<(), String> {
    conn.execute(
        "INSERT INTO comic_items (item_id, series, volume, page_count)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(item_id) DO UPDATE SET
           series = COALESCE(excluded.series, comic_items.series),
           volume = COALESCE(excluded.volume, comic_items.volume),
           page_count = COALESCE(excluded.page_count, comic_items.page_count)",
        params![item_id, d.series(), d.volume(), d.page_count()],
    )
    .map_err(|e| format!("写入 comic_items 失败 id={item_id}: {e}"))?;
    Ok(())
}

/// 归一化绝对路径为匹配键：反斜杠→斜杠、折叠重复斜杠、小写
pub fn normalize_key(p: &str) -> String {
    let mut s = p.replace('\\', "/").to_lowercase();
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    s
}

/// 全库（或限定类型）绝对路径 → item_id 映射，供库导入匹配
pub fn abs_path_map(
    conn: &Connection,
    type_name: &str,
) -> Result<std::collections::HashMap<String, i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.id, r.path, i.rel_path FROM items i
             JOIN roots r ON r.id = i.root_id
             WHERE i.type = ?1 AND i.missing = 0",
        )
        .map_err(|e| format!("查询 items 失败: {e}"))?;
    let rows = stmt
        .query_map([type_name], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("查询 items 失败: {e}"))?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (id, root_path, rel_path) = row.map_err(|e| format!("读取行失败: {e}"))?;
        let abs = PathBuf::from(root_path).join(rel_path);
        map.insert(normalize_key(&abs.to_string_lossy()), id);
    }
    Ok(map)
}
