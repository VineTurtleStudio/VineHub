//! 查重：按「大小 → 全文 SHA-256」两级分组找出内容完全相同的文件。
//! 纯逻辑，零 Tauri 依赖；进度与取消通过回调 / AtomicBool 注入。

use crate::fsops;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// 一组重复文件（相同 size + 相同 SHA-256，≥2 个）
#[derive(Debug, Clone, Serialize)]
pub struct DupGroup {
    pub size: u64,
    /// 内容 SHA-256（hex）
    pub hash: String,
    /// 相对根目录的路径（正斜杠，已按字典序排序）
    pub files: Vec<String>,
}

/// 查找根目录下的重复文件。
///
/// - 跳过：重连点（symlink/junction）、`desktop.ini` / `Thumbs.db`、空文件（size 0）；
/// - 两级分组：先按 size 分组（仅 size 出现 ≥2 的才是候选），再对候选做全文哈希；
/// - `on_progress(phase, index, total, rel)`：phase = `scan`（total=0 未知）| `hash`；
/// - `cancel` 置位后尽快返回**已得结果**（哈希阶段被取消时返回已算完的分组）。
pub fn find_duplicates(
    root: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&str, usize, usize, &str),
) -> Vec<DupGroup> {
    // 1) 递归收集普通文件
    let mut files: Vec<(String, u64)> = Vec::new();
    walk(root, root, cancel, &mut on_progress, &mut files);

    // 2) 按大小分组，仅 size 出现 ≥2 的进入候选
    let mut by_size: HashMap<u64, Vec<String>> = HashMap::new();
    for (rel, size) in files {
        by_size.entry(size).or_default().push(rel);
    }
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for (size, rels) in by_size {
        if rels.len() < 2 {
            continue;
        }
        for rel in rels {
            candidates.push((size, rel));
        }
    }
    candidates.sort();

    // 3) 候选逐个全文哈希 → (size, hash) 分组
    let mut by_key: HashMap<(u64, String), Vec<String>> = HashMap::new();
    let total = candidates.len();
    for (i, (size, rel)) in candidates.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        on_progress("hash", i + 1, total, rel);
        if let Some(hash) = hash_file(&root.join(rel)) {
            by_key
                .entry((*size, hash))
                .or_default()
                .push(rel.clone());
        }
    }

    // 4) 过滤 ≥2，组内排序（保证「保留首个」确定），按大小降序
    let mut groups: Vec<DupGroup> = by_key
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .map(|((size, hash), mut files)| {
            files.sort();
            DupGroup { size, hash, files }
        })
        .collect();
    groups.sort_by(|a, b| b.size.cmp(&a.size));
    groups
}

fn walk(
    dir: &Path,
    root: &Path,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(&str, usize, usize, &str),
    out: &mut Vec<(String, u64)>,
) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(meta) = e.metadata() else {
            continue;
        };
        if fsops::is_reparse(&meta) {
            continue;
        }
        if meta.is_dir() {
            walk(&e.path(), root, cancel, on_progress, out);
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "desktop.ini" || name == "Thumbs.db" {
            continue;
        }
        let size = meta.len();
        if size == 0 {
            continue;
        }
        let rel = rel_of(&e.path(), root);
        on_progress("scan", out.len() + 1, 0, &rel);
        out.push((rel, size));
    }
}

/// 绝对路径 → 相对 root 的正斜杠路径
fn rel_of(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// 流式 SHA-256（128 KiB 分块）；打不开 / 读失败返回 None（该文件跳过，不参与分组）
fn hash_file(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(format!("{:x}", h.finalize()))
}
