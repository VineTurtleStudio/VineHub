use super::filter::{is_reserved_root, rel_of, Filter};
use super::{normalize_key, systemtime_to_ns, E, FileMeta};
use std::collections::BTreeMap;
use std::path::Path;
use walkdir::WalkDir;

pub const TEMP_SUFFIX: &str = ".vinesync.tmp";

/// 扫描结果：文件清单 + 被跳过的不可访问路径
pub struct ScanOutput {
    /// `key -> FileMeta`（key 为规范化相对路径）
    pub files: BTreeMap<String, FileMeta>,
    /// 因权限不足/扫描期间被移除等原因跳过的子树根或文件（已规范化），
    /// 供目标端剔除对应副本，避免被误判为“已删除”
    pub skipped: Vec<String>,
    /// 跳过的符号链接 / 目录联接数量（不跟随，其中的文件不会进入备份）
    pub symlinks: usize,
    /// 规范化 key 冲突（仅大小写不同的两个路径）样本：
    /// 在大小写不敏感的目标卷上无法共存，只能保留其一
    pub case_collisions: Vec<String>,
}

impl Default for ScanOutput {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            skipped: Vec::new(),
            symlinks: 0,
            case_collisions: Vec::new(),
        }
    }
}

/// 从目标端扫描结果中剔除 `skipped` 覆盖的子树与文件，
/// 防止源端不可访问的路径在镜像/保守策略下误删目标副本
pub fn prune_under(map: &mut BTreeMap<String, FileMeta>, skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    map.retain(|k, _| {
        !skipped.iter().any(|s| {
            k == s || (k.starts_with(s.as_str()) && k.as_bytes().get(s.len()) == Some(&b'/'))
        })
    });
}

/// 扫描目录树，每收录一个文件回调一次 `on_file(rel_path)`（供进度上报）。
/// - `clean_temp`：目标端扫描时清理残留的 `.vinesync.tmp` 临时文件
/// - 根目录自身的读取错误视为致命；子树级错误（如权限拒绝）记入
///   `skipped` 并跳过，不再中断整个扫描
pub fn scan_with_progress(
    root: &Path,
    filter: &Filter,
    clean_temp: bool,
    mut on_file: impl FnMut(&str),
) -> E<ScanOutput> {
    let mut out = ScanOutput::default();
    if !root.exists() {
        return Ok(out);
    }
    // filter_entry 是闭包，用 Cell 统计既不借走 out，也能在结束后汇总
    let symlinks = std::cell::Cell::new(0usize);

    let iter = WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            let ft = entry.file_type();
            // 重解析点一律不跟随、不进入清单。符号链接与目录联接（junction）
            // 均返回 is_symlink()==true（实测含 DIRECTORY 属性的 junction 亦然），
            // 只有此一项检查即可防止外部目录树/循环联接被纳入备份。
            if ft.is_symlink() {
                symlinks.set(symlinks.get() + 1);
                return false;
            }
            // 文件在主循环里过滤（连同 rel_of 一起做），避免每文件重复计算相对路径
            if !ft.is_dir() {
                return true;
            }
            let path = entry.path();
            // 目标端扫描：上次中断留下的 `<文件>.vinesync.tmp` 目录是垃圾，
            // 不进清单（否则会被当成源端已删除而清理，或挡住同名文件的复制）。
            if clean_temp && path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(TEMP_SUFFIX)) {
                let _ = std::fs::remove_dir_all(path);
                return false;
            }
            let rel = match rel_of(root, path) {
                Some(r) => r,
                None => return false,
            };
            if rel.is_empty() {
                return true;
            }
            if is_reserved_root(&rel) {
                return false;
            }
            filter.allow_dir(&rel)
        });

    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // 根目录本身不可读 → 致命（空结果会引发误删）；
                // 子树级错误（权限拒绝、条目在扫描期间被移除等）→ 记录并跳过
                if e.depth() > 0 {
                    if let Some(p) = e.path() {
                        if let Some(rel) = rel_of(root, p) {
                            out.skipped.push(normalize_key(&rel));
                        }
                    }
                    continue;
                }
                return Err(crate::msg!("扫描 {}: {e}", "Scan {}: {e}", root.display()));
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match rel_of(root, entry.path()) {
            Some(r) => r,
            None => continue,
        };
        if rel.is_empty() || is_reserved_root(&rel) {
            continue;
        }
        if !filter.allow_file(&rel) {
            continue;
        }
        if clean_temp && rel.ends_with(TEMP_SUFFIX) {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        let md = match entry.metadata() {
            Ok(md) => md,
            Err(_) => {
                out.skipped.push(normalize_key(&rel));
                continue;
            }
        };
        let mtime_ns = md.modified().map(systemtime_to_ns).unwrap_or(0);
        on_file(&rel);
        let key = normalize_key(&rel);
        // 仅大小写不同的两个路径在目标卷上无法共存：保留第一个，并记录样本
        if let Some(prev) = out.files.get(&key) {
            if prev.rel != rel {
                if out.case_collisions.len() < 20 {
                    out.case_collisions.push(format!("{} ↔ {}", prev.rel, rel));
                }
                continue;
            }
        }
        out.files.insert(key, FileMeta { rel, size: md.len(), mtime_ns });
    }
    out.symlinks = symlinks.get();
    Ok(out)
}
