use super::scan::TEMP_SUFFIX;
use super::{ns_to_systemtime, E};
use std::path::{Path, PathBuf};

/// 安全复制：写入临时文件 -> 校验大小 -> 回写源修改时间 -> 原子重命名覆盖
pub fn safe_copy(src: &Path, dst: &Path, mtime_ns: i64) -> E<()> {
    safe_copy_hook(src, dst, mtime_ns, || Ok(()))
}

/// 同 `safe_copy`，但在最终覆盖目标前执行 `before_overwrite` 钩子
///（用于把「先移旧文件入版本备份」推迟到新文件已暂存并校验之后）。
/// 钩子失败时清理临时文件并返回错误，目标文件保持原样。
pub fn safe_copy_hook(
    src: &Path,
    dst: &Path,
    mtime_ns: i64,
    before_overwrite: impl FnOnce() -> E<()>,
) -> E<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::msg!("创建目录失败 {}: {e}", "Failed to create directory {}: {e}", parent.display())
        })?;
    }

    let mut tmp_name = dst.as_os_str().to_owned();
    tmp_name.push(TEMP_SUFFIX);
    let tmp = PathBuf::from(tmp_name);
    let _ = std::fs::remove_file(&tmp);

    std::fs::copy(src, &tmp).map_err(|e| crate::msg!("复制失败: {e}", "Copy failed: {e}"))?;

    let src_size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(u64::MAX);
    let tmp_size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if src_size != tmp_size {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::msg!(
            "复制校验失败（大小不一致）: {}",
            "Copy verification failed (size mismatch): {}",
            src.display()
        ));
    }

    let _ = set_mtime(&tmp, mtime_ns);

    if let Err(e) = before_overwrite() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    rename_replace(&tmp, dst)
}

fn set_mtime(path: &Path, ns: i64) -> std::io::Result<()> {
    let file = std::fs::File::options().write(true).open(path)?;
    file.set_modified(ns_to_systemtime(ns))?;
    Ok(())
}

/// 原子覆盖重命名；Windows 下目标只读时清除属性后重试
pub fn rename_replace(from: &Path, to: &Path) -> E<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(first) => {
            clear_readonly(to);
            clear_readonly(from);
            std::fs::rename(from, to).map_err(|e| {
                let _ = std::fs::remove_file(from);
                crate::msg!(
                    "覆盖目标失败 {}: {e}（首次: {first}）",
                    "Overwrite failed {}: {e} (first attempt: {first})",
                    to.display()
                )
            })
        }
    }
}

pub fn clear_readonly(path: &Path) {
    if let Ok(md) = std::fs::metadata(path) {
        let mut perms = md.permissions();
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
}
