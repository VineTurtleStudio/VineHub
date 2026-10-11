//! 缩略图（纯逻辑，零 Tauri）：图片缩放 + 视频抽帧，落派生目录 thumbs/。
//! 文件名 `<item_id>-<mtime_ms>.jpg`：mtime 变化即重新生成，旧文件顺带清理。
//! 只读原始媒体；生成成功置 `items.has_cover=1`。视频依赖外部 ffmpeg（缺失报错不崩溃）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::repo;

const MAX_EDGE: u32 = 320;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ThumbResult {
    pub path: String,
    pub created: bool,
}

/// 派生数据基目录：settings `derived_dir`（空则回落调用方给的默认目录）
pub fn derived_base(conn: &Connection, default_dir: &Path) -> Result<PathBuf, String> {
    let custom = repo::get_setting_value(conn, "derived_dir").map_err(|e| e.to_string())?;
    if custom.trim().is_empty() {
        Ok(default_dir.to_path_buf())
    } else {
        Ok(PathBuf::from(custom.trim()))
    }
}

/// 派生数据根：settings `derived_dir`（空则回落调用方给的默认目录）+ thumbs/
pub fn derived_root(conn: &Connection, default_dir: &Path) -> Result<PathBuf, String> {
    let thumbs = derived_base(conn, default_dir)?.join("thumbs");
    fs::create_dir_all(&thumbs).map_err(|e| format!("创建缩略图目录失败: {e}"))?;
    Ok(thumbs)
}

fn target_path(thumbs: &Path, item_id: i64, mtime_ms: i64) -> PathBuf {
    thumbs.join(format!("{item_id}-{mtime_ms}.jpg"))
}

/// 清掉同一 item 的旧 mtime 缩略图（保留 current）
fn cleanup_stale(thumbs: &Path, item_id: i64, keep: &Path) {
    let prefix = format!("{item_id}-");
    let Ok(rd) = fs::read_dir(thumbs) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p == keep {
            continue;
        }
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.starts_with(&prefix) && name.ends_with(".jpg") {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

/// 取/建缩略图。返回绝对路径；已存在且新鲜则不重复生成。
/// `default_derived` = AppData/derived（未设置 derived_dir 时的回落）。
/// `ffmpeg` = ffmpeg 可执行文件（override 路径或 PATH 名 "ffmpeg"）。
pub fn ensure_thumb(
    conn: &Connection,
    item_id: i64,
    default_derived: &Path,
    ffmpeg: &str,
) -> Result<ThumbResult, String> {
    let (root_path, rel_path, item_type, mtime_ms): (String, String, String, i64) = conn
        .query_row(
            "SELECT r.path, i.rel_path, i.type, i.mtime
             FROM items i JOIN roots r ON r.id = i.root_id
             WHERE i.id = ?1",
            params![item_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| format!("条目不存在: {e}"))?;

    let thumbs = derived_root(conn, default_derived)?;
    let target = target_path(&thumbs, item_id, mtime_ms);
    if target.is_file() {
        return Ok(ThumbResult {
            path: target.to_string_lossy().into_owned(),
            created: false,
        });
    }

    let mut src = PathBuf::from(&root_path);
    for part in rel_path.split('/') {
        if !part.is_empty() {
            src.push(part);
        }
    }
    if !src.is_file() {
        return Err(format!("源文件不存在: {}", src.display()));
    }
    generate(&src, &item_type, &target, ffmpeg, conn, item_id)
}

fn generate(
    src: &Path,
    item_type: &str,
    target: &Path,
    ffmpeg: &str,
    conn: &Connection,
    item_id: i64,
) -> Result<ThumbResult, String> {
    let tmp = target.with_extension("tmp.jpg");
    match item_type {
        "image" => resize_image(src, &tmp)?,
        "video" => extract_frame(src, &tmp, ffmpeg)?,
        other => return Err(format!("该类型暂不支持缩略图: {other}")),
    }
    fs::rename(&tmp, target).map_err(|e| format!("写入缩略图失败: {e}"))?;
    conn.execute(
        "UPDATE items SET has_cover = 1, updated_at = datetime('now') WHERE id = ?1",
        params![item_id],
    )
    .map_err(|e| format!("更新 has_cover 失败: {e}"))?;
    if let Some(dir) = target.parent() {
        cleanup_stale(dir, item_id, target);
    }
    Ok(ThumbResult {
        path: target.to_string_lossy().into_owned(),
        created: true,
    })
}

fn resize_image(src: &Path, out: &Path) -> Result<(), String> {
    let img = image::open(src).map_err(|e| format!("解码图片失败: {e}"))?;
    let thumb = img.thumbnail(MAX_EDGE, MAX_EDGE);
    thumb
        .save_with_format(out, image::ImageFormat::Jpeg)
        .map_err(|e| format!("编码缩略图失败: {e}"))?;
    Ok(())
}

fn run_ffmpeg(ffmpeg: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd.output()
}

fn extract_frame(src: &Path, out: &Path, ffmpeg: &str) -> Result<(), String> {
    let src_s = src.to_string_lossy();
    let out_s = out.to_string_lossy();
    // 先取 3 秒处（避开片头黑场），太短的片子回落 0 秒
    for ss in ["3", "0"] {
        let args = [
            "-y",
            "-ss",
            ss,
            "-i",
            src_s.as_ref(),
            "-frames:v",
            "1",
            "-vf",
            "scale=320:-2",
            "-q:v",
            "4",
            out_s.as_ref(),
        ];
        match run_ffmpeg(ffmpeg, &args) {
            Ok(output)
                if output.status.success()
                    && fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false) =>
            {
                return Ok(());
            }
            _ => {
                let _ = fs::remove_file(out);
            }
        }
    }
    Err("ffmpeg 抽帧失败（检查 ffmpeg 是否可用）".into())
}

/// 探测 ffmpeg：settings `ffmpeg_path` 覆盖，否则 PATH
pub fn ffmpeg_program(overrides: &HashMap<String, String>) -> String {
    overrides
        .get(&crate::env::override_key("ffmpeg"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "ffmpeg".to_string())
}

/// has_cover 是否已有缩略图文件（列表圆点用，不触发生成）
pub fn thumb_exists(
    conn: &Connection,
    item_id: i64,
    default_derived: &Path,
) -> Result<bool, String> {
    let mtime: i64 = conn
        .query_row(
            "SELECT mtime FROM items WHERE id = ?1",
            params![item_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("条目不存在: {e}"))?;
    let thumbs = derived_root(conn, default_derived)?;
    Ok(target_path(&thumbs, item_id, mtime).is_file())
}

/// 只查路径不生成（已生成过才返回）
pub fn existing_thumb(
    conn: &Connection,
    item_id: i64,
    default_derived: &Path,
) -> Result<Option<String>, String> {
    let mtime: Option<i64> = conn
        .query_row(
            "SELECT mtime FROM items WHERE id = ?1",
            params![item_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(mtime) = mtime else {
        return Ok(None);
    };
    let thumbs = derived_root(conn, default_derived)?;
    let p = target_path(&thumbs, item_id, mtime);
    if p.is_file() {
        Ok(Some(p.to_string_lossy().into_owned()))
    } else {
        Ok(None)
    }
}
