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
    let parent = dst.parent().ok_or_else(|| {
        crate::msg!("目标路径没有父目录: {}", "Destination has no parent: {}", dst.display())
    })?;
    ensure_dir(parent)?;

    let tmp = tmp_path(dst);
    // 上次中断/手工建的同名临时对象必须彻底清掉：
    // 只读的 tmp 文件与同名 tmp 目录会让 `remove_file` 静默失败，
    // 随后的复制打在它上面就是「拒绝访问 (os error 5)」，且每次都复现。
    clear_tmp(&tmp)?;

    // 源文件只读时 `std::fs::copy` 会把只读属性一起带到 tmp，
    // 之后打开写入回写 mtime 会失败（静默），目标 mtime 与原文件不一致，
    // 下一轮又判定为「需要更新」，形成永远重复复制。故先清属性再回写时间。
    let src_readonly = is_readonly(src);
    copy_file(src, &tmp)?;

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

    clear_readonly(&tmp);
    if let Err(e) = set_mtime(&tmp, mtime_ns) {
        log::warn!("回写修改时间失败 {}: {e}", tmp.display());
    }

    if let Err(e) = before_overwrite() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    rename_replace(&tmp, dst)?;
    // 保留源文件的只读语义（覆盖时 rename_replace 会先清属性）
    if src_readonly {
        set_readonly(dst);
    }
    Ok(())
}

fn tmp_path(dst: &Path) -> PathBuf {
    let mut name = dst.as_os_str().to_owned();
    name.push(TEMP_SUFFIX);
    PathBuf::from(name)
}

/// 清理目标临时对象：文件（含只读）直接删，同名目录整棵删。
fn clear_tmp(tmp: &Path) -> E<()> {
    let Ok(md) = std::fs::metadata(tmp) else { return Ok(()) };
    if md.is_dir() {
        std::fs::remove_dir_all(tmp).map_err(|e| {
            crate::msg!(
                "清理残留临时目录失败 {}: {e}（请手工删除该目录后重试）",
                "Failed to remove leftover temp directory {}: {e} (delete it manually and retry)",
                tmp.display()
            )
        })?;
        return Ok(());
    }
    clear_readonly(tmp);
    match std::fs::remove_file(tmp) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::msg!(
            "清理残留临时文件失败 {}: {e}（文件可能被占用或只读）",
            "Failed to remove leftover temp file {}: {e} (locked or read-only)",
            tmp.display()
        )),
    }
}

/// 目标目录可写性预检：在目录内创建并删除一个探针文件。
///
/// 目的：换机器/换盘后目标目录常常只对当前用户「可读不可写」（旧机器 SID 的
/// ACL、只读介质、受控文件夹访问），这种问题会把整轮同步拖成几十个
/// 「拒绝访问 (os error 5)」。放在预览阶段探测，可以提前给出准确目录名。
/// 目录不存在时返回 Ok（由同步阶段创建并报告失败原因）。
pub fn probe_writable(dir: &Path) -> E<()> {
    let Ok(md) = std::fs::metadata(dir) else { return Ok(()) };
    if !md.is_dir() {
        return Err(crate::msg!(
            "目标路径不是目录: {}",
            "Destination is not a directory: {}",
            dir.display()
        ));
    }
    let probe = dir.join(format!(".vinesync-probe-{}.tmp", std::process::id()));
    let create = |p: &Path| std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(p);
    match with_plain_retry(&probe, create) {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&probe);
            Err(crate::msg!(
                "目标目录不可写 {}: {e}（请检查该目录是否只读、被占用，或在 属性→安全 中给当前用户「完全控制」）",
                "Destination is not writable {}: {e} (check read-only/ownership, or grant your account Full control)",
                dir.display()
            ))
        }
    }
}

fn is_readonly(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.permissions().readonly()).unwrap_or(false)
}

fn set_readonly(path: &Path) {
    if let Ok(md) = std::fs::metadata(path) {
        let mut perms = md.permissions();
        perms.set_readonly(true);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// 目标端目录位置被一个文件占住时给出可操作的提示，而不是裸 os error。
pub fn ensure_dir(dir: &Path) -> E<()> {
    match with_plain_retry(dir, |d| std::fs::create_dir_all(d)) {
        Ok(()) => Ok(()),
        Err(e) => {
            if std::fs::metadata(dir).map(|m| !m.is_dir()).unwrap_or(false) {
                return Err(crate::msg!(
                    "创建目录失败 {}: 目标端该路径已存在且不是目录（是文件），请确认后删除或改名（{e}）",
                    "Failed to create directory {}: a file occupies that path on the destination ({e})",
                    dir.display()
                ));
            }
            Err(crate::msg!(
                "创建目录失败 {}: {e}（目标目录可能只读、被占用或权限受限，可检查该目录的安全设置）",
                "Failed to create directory {}: {e} (target may be read-only, locked, or permission-restricted)",
                dir.display()
            ))
        }
    }
}

/// 复制到临时文件；权限类错误时退回非 verbatim 路径重试一次。
/// 某些筛选驱动 / 网络重定向 / 云盘映射对 `\\?\` 形式的路径返回「拒绝访问」。
fn copy_file(src: &Path, tmp: &Path) -> E<()> {
    match with_plain_retry(tmp, |t| std::fs::copy(src, t).map(|_| ())) {
        Ok(()) => Ok(()),
        Err(e) => Err(crate::msg!(
            "复制失败: {e}（源文件可能被占用/无读权限，或目标目录只读）",
            "Copy failed: {e} (source locked/unreadable, or destination read-only)"
        )),
    }
}

/// 用普通（去 verbatim）路径重试一次权限类失败。返回首次的错误以便提示原文。
fn with_plain_retry<T>(path: &Path, f: impl Fn(&Path) -> std::io::Result<T>) -> std::io::Result<T> {
    match f(path) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => match plain_path(path) {
            Some(alt) => f(&alt).map_err(|_| e),
            None => Err(e),
        },
        other => other,
    }
}

/// `\\?\C:\x` → `C:\x`；`\\?\UNC\server\share\x` → `\\server\share\x`。
/// 非 verbatim 路径返回 None。
pub fn plain_path(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        const VERBATIM: [u16; 4] = [92, 92, 63, 92]; // \\?\
        if wide.len() < 4 || wide[..4] != VERBATIM {
            return None;
        }
        const UNC: [u16; 4] = [85, 78, 67, 92]; // "UNC\"
        if wide.len() >= 8 && wide[4..8] == UNC {
            let mut out: Vec<u16> = vec![92, 92];
            out.extend_from_slice(&wide[8..]);
            return Some(PathBuf::from(OsString::from_wide(&out)));
        }
        Some(PathBuf::from(OsString::from_wide(&wide[4..])))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

fn set_mtime(path: &Path, ns: i64) -> std::io::Result<()> {
    let file = std::fs::File::options().write(true).open(path)?;
    file.set_modified(ns_to_systemtime(ns))?;
    Ok(())
}

/// 原子覆盖重命名；Windows 下目标只读时清除属性后重试
pub fn rename_replace(from: &Path, to: &Path) -> E<()> {
    match with_plain_retry(to, |dst| std::fs::rename(from, dst)) {
        Ok(()) => Ok(()),
        Err(first) => {
            clear_readonly(to);
            clear_readonly(from);
            match with_plain_retry(to, |dst| std::fs::rename(from, dst)) {
                Ok(()) => Ok(()),
                Err(e) => {
                    let _ = std::fs::remove_file(from);
                    Err(crate::msg!(
                        "覆盖目标失败 {}: {e}（首次: {first}）",
                        "Overwrite failed {}: {e} (first attempt: {first})",
                        to.display()
                    ))
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vs_copy_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn plain_path_strips_verbatim_prefix() {
        assert_eq!(plain_path(Path::new(r"C:\docs")), None);
        assert_eq!(plain_path(Path::new(r"relative\x")), None);
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\docs\a.txt")),
            Some(PathBuf::from(r"C:\docs\a.txt"))
        );
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\nas\share\a.txt")),
            Some(PathBuf::from(r"\\nas\share\a.txt"))
        );
    }

    /// 上次中断留下的 `<文件>.vinesync.tmp` 目录不得永久挡住复制
    #[test]
    fn stale_tmp_directory_is_cleared() {
        let dir = tmp_dir("tmpdir");
        let src = dir.join("src.txt");
        let dst = dir.join("out.txt");
        std::fs::write(&src, "NEW").unwrap();
        let stale = tmp_path(&dst);
        std::fs::create_dir_all(stale.join("nested")).unwrap();
        std::fs::write(stale.join("nested").join("junk.txt"), "x").unwrap();

        safe_copy(&src, &dst, 0).expect("残留 tmp 目录应被清理后正常复制");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "NEW");
        assert!(!stale.exists(), "残留 tmp 目录应被删除");
    }

    /// 只读的残留 tmp 文件同样不能挡住复制
    #[test]
    fn stale_readonly_tmp_file_is_cleared() {
        let dir = tmp_dir("tmpro");
        let src = dir.join("src.txt");
        let dst = dir.join("out.txt");
        std::fs::write(&src, "NEW").unwrap();
        let stale = tmp_path(&dst);
        std::fs::write(&stale, "OLD").unwrap();
        let mut perms = std::fs::metadata(&stale).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&stale, perms).unwrap();

        safe_copy(&src, &dst, 0).expect("只读残留 tmp 应被清理后正常复制");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "NEW");
    }

    /// 只读源文件：目标要拿到同样的 mtime（否则每轮都重复复制）且保留只读语义
    #[test]
    fn readonly_source_keeps_mtime_and_flag() {
        let dir = tmp_dir("rosrc");
        let src = dir.join("src.txt");
        let dst = dir.join("out.txt");
        std::fs::write(&src, "CONTENT").unwrap();
        let mut perms = std::fs::metadata(&src).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&src, perms).unwrap();
        let mtime = super::super::systemtime_to_ns(
            std::fs::metadata(&src).unwrap().modified().unwrap(),
        );

        for _ in 0..2 {
            safe_copy(&src, &dst, mtime).expect("只读源应可复制");
        }
        let dst_md = std::fs::metadata(&dst).unwrap();
        assert!(dst_md.permissions().readonly(), "应保留源文件只读语义");
        assert_eq!(
            super::super::systemtime_to_ns(dst_md.modified().unwrap()),
            mtime,
            "只读源文件的时间戳必须回写成功，否则每轮都会重复复制"
        );
        // 第二次复制也不会因为目标只读而失败
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "CONTENT");
    }

    /// 目标目录可写性预检：正常目录通过、缺失目录跳过、被文件占位时给出明确提示
    #[test]
    fn probe_writable_cases() {        let dir = tmp_dir("probe");
        assert!(probe_writable(&dir).is_ok(), "可写目录应通过预检");
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_none(),
            "预检不应留下探针文件"
        );

        assert!(probe_writable(&dir.join("missing")).is_ok(), "缺失目录交由同步阶段创建");

        let occupied = dir.join("occupied");
        std::fs::write(&occupied, "x").unwrap();
        let err = probe_writable(&occupied).unwrap_err();
        assert!(
            err.contains("不是目录") || err.contains("not a directory"),
            "预检应指出该路径不是目录: {err}"
        );
    }

    /// 目标端把目录位置占成文件时，报错要能指导用户处理
    #[test]
    fn dir_occupied_by_file_reports_actionable_error() {
        let dir = tmp_dir("dirconflict");
        let src = dir.join("src.txt");
        std::fs::write(&src, "X").unwrap();
        let blocking = dir.join("sub");
        std::fs::write(&blocking, "I am a file").unwrap();

        let err = safe_copy(&src, &blocking.join("a.txt"), 0).unwrap_err();
        assert!(
            err.contains("不是目录") || err.contains("not a directory"),
            "错误信息应指出「该路径是文件」: {err}"
        );
    }
}
