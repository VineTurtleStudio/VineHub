use super::copy::clear_readonly;
use super::systemtime_to_ns;
use super::E;
use chrono::{Duration, Local};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const VERSION_DIR: &str = "版本备份";

/// 将目标端文件移动到版本备份目录，返回其相对路径
/// 目录结构：`<dest>/版本备份/<相对子路径>/<文件名>_<时间戳>.<扩展名>`
/// `ts_fmt` 为 chrono 格式串（配置 `versionTimestampFormat`），结果会剔除
/// 文件名非法字符；移动后将修改时间更新为当前时间（便于按天数清理）；
/// `max_count > 0` 时按份数上限裁剪该文件的历史版本（按 mtime 取最旧）。
pub fn move_to_version(dest_root: &Path, dst_rel: &str, max_count: u32, ts_fmt: &str) -> E<String> {
    let src = dest_root.join(dst_rel);
    if !src.exists() {
        return Err(crate::msg!("待备份文件不存在: {dst_rel}", "File to back up not found: {dst_rel}"));
    }

    let rel_path = Path::new(dst_rel);
    let file_name = rel_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| crate::msg!("无效路径: {dst_rel}", "Invalid path: {dst_rel}"))?;
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    let ext = Path::new(file_name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    let ts = version_timestamp(ts_fmt);
    let mut ver_rel = PathBuf::from(VERSION_DIR);
    if let Some(parent) = rel_path.parent() {
        if !parent.as_os_str().is_empty() {
            ver_rel.push(parent);
        }
    }

    let mut candidate = ver_rel.join(format!("{stem}_{ts}{ext}"));
    let mut n = 1u32;
    while dest_root.join(&candidate).exists() && n < 10_000 {
        candidate = ver_rel.join(format!("{stem}_{ts}_{n}{ext}"));
        n += 1;
    }

    let target = dest_root.join(&candidate);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::msg!(
                "创建版本目录失败 {}: {e}",
                "Failed to create version directory {}: {e}",
                parent.display()
            )
        })?;
    }

    if let Err(e) = std::fs::rename(&src, &target) {
        clear_readonly(&src);
        std::fs::rename(&src, &target).map_err(|e2| {
            crate::msg!(
                "移入版本备份失败 {}: {e} / {e2}",
                "Failed to move to version backup {}: {e} / {e2}",
                src.display()
            )
        })?;
    }

    // 以备份时间作为版本文件的修改时间（便于按天清理）。
    // 只读文件先清除只读属性，否则打开写入会失败 → mtime 保留源文件旧时间，
    // 会被按天清理误删刚备份的历史
    clear_readonly(&target);
    if let Ok(f) = std::fs::File::options().write(true).open(&target) {
        let now_ns = systemtime_to_ns(std::time::SystemTime::now());
        let _ = f.set_modified(super::ns_to_systemtime(now_ns));
    }

    // 按份数上限裁剪该文件的历史版本
    if max_count > 0 {
        if let Some(ver_dir) = target.parent() {
            enforce_max_count(ver_dir, stem, &ext, max_count);
        }
    }

    Ok(candidate.to_string_lossy().replace('/', "\\"))
}

/// 按配置格式生成时间戳并剔除文件名非法字符；结果为空时退回默认格式
pub(crate) fn version_timestamp(fmt: &str) -> String {
    let raw = Local::now().format(fmt).to_string();
    let clean: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32
            {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = clean.trim_matches(|c: char| c == '-' || c == '.' || c == ' ');
    if trimmed.is_empty() {
        Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
    } else {
        clean
    }
}

/// 保留同一原文件最近 `max_count` 份历史版本，删除更旧的。
/// 以文件修改时间排序（备份时已写入备份时刻），mtime 相同时按名字
/// 字典序兜底；时间戳格式可自定义，因此不再依赖名字顺序。
fn enforce_max_count(ver_dir: &Path, stem: &str, ext: &str, max_count: u32) {
    let prefix = format!("{stem}_");
    let mut versions: Vec<(String, i64)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(ver_dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !entry.path().is_file() {
                continue;
            }
            let Some(rest) = name.strip_prefix(prefix.as_str()) else { continue };
            if !rest.ends_with(ext) || rest.len() <= ext.len() {
                continue;
            }
            let stamp = &rest[..rest.len() - ext.len()];
            // 时间戳以数字开头，借此排除 `report_v2_…` 这类撞前缀的其他文件
            if stamp.starts_with(|c: char| c.is_ascii_digit()) {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(super::systemtime_to_ns)
                    .unwrap_or(0);
                versions.push((name, mtime));
            }
        }
    }
    if versions.len() as u32 <= max_count {
        return;
    }
    versions.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (name, _) in versions.iter().take(versions.len() - max_count as usize) {
        let _ = std::fs::remove_file(ver_dir.join(name));
    }
}

/// 清理版本备份中超过 `keep_days` 天的文件，返回删除数量；同时清理空目录
pub fn cleanup(dest_root: &Path, keep_days: u32) -> E<u32> {
    let dest_root = crate::core::to_long_path(dest_root);
    let dest_root = dest_root.as_path();
    let vroot = dest_root.join(VERSION_DIR);
    if !vroot.exists() {
        return Ok(0);
    }

    let cutoff = Local::now() - Duration::days(keep_days as i64);
    let cutoff_ns = cutoff.timestamp_nanos_opt().unwrap_or(i64::MAX);
    let mut removed = 0u32;

    for entry in WalkDir::new(&vroot).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let mtime_ns = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(super::systemtime_to_ns)
            .unwrap_or(0);
        if mtime_ns < cutoff_ns {
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }

    prune_empty_dirs(&vroot);
    Ok(removed)
}

/// 递归删除空目录（含根目录自身，若为空）
fn prune_empty_dirs(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                prune_empty_dirs(&p);
            }
        }
    }
    let empty = std::fs::read_dir(dir).map(|mut rd| rd.next().is_none()).unwrap_or(false);
    if empty {
        let _ = std::fs::remove_dir(dir);
    }
}
