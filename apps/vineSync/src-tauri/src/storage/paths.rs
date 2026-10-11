//! 数据目录解析：便携模式（exe 旁存在 `vinesync.portable` 标记）时数据随程序目录，
//! 否则使用系统用户目录（Windows: `%APPDATA%\studio.vineturtle.vinesync`）。

use std::path::{Path, PathBuf};

/// 便携模式标记文件（与 exe 同目录）
pub const MARKER_FILE: &str = "vinesync.portable";
/// 便携模式数据子目录名
pub const PORTABLE_DIR: &str = "portable_data";

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// exe 旁是否存在便携模式标记
pub fn marker_exists() -> bool {
    exe_dir()
        .map(|d| d.join(MARKER_FILE).exists())
        .unwrap_or(false)
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 便携模式数据目录：标记存在且目录可写时返回 `Some(<exe_dir>/portable_data)`。
/// 只读介质等不可写场景回退到用户目录。
pub fn portable_data_dir() -> Option<PathBuf> {
    if !marker_exists() {
        return None;
    }
    let dir = exe_dir()?.join(PORTABLE_DIR);
    if writable(&dir) {
        Some(dir)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_marker_means_not_portable() {
        // 测试二进制的 target 目录下不会存在标记文件
        assert!(!marker_exists());
        assert!(portable_data_dir().is_none());
    }

    #[test]
    fn marker_file_name_stable() {
        assert_eq!(MARKER_FILE, "vinesync.portable");
        assert_eq!(PORTABLE_DIR, "portable_data");
    }
}
