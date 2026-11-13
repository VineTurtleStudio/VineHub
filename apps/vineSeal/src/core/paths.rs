use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    SamePath,
    InsideSource,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::SamePath => write!(f, "源与目标路径相同"),
            PathError::InsideSource => write!(f, "目标位于源目录内部"),
        }
    }
}

impl std::error::Error for PathError {}

pub fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

pub fn to_long_path(p: &Path) -> PathBuf {
    let abs = absolutize(p);
    let s = abs.to_string_lossy().replace('/', "\\");
    if s.starts_with(r"\\?\") {
        return PathBuf::from(s);
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{rest}"))
    } else {
        PathBuf::from(format!(r"\\?\{s}"))
    }
}

pub fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

fn normalize(p: &Path) -> PathBuf {
    let s = p
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    PathBuf::from(s)
}

pub fn ensure_not_inside(dest: &Path, src: &Path) -> Result<(), PathError> {
    let d = normalize(&absolutize(dest));
    let s = normalize(&absolutize(src));
    if d == s {
        return Err(PathError::SamePath);
    }
    if d.starts_with(&s) {
        return Err(PathError::InsideSource);
    }
    Ok(())
}

pub fn same_drive(a: &Path, b: &Path) -> bool {
    let da = absolutize(a);
    let db = absolutize(b);
    match (da.components().next(), db.components().next()) {
        (Some(ca), Some(cb)) => ca
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&cb.as_os_str().to_string_lossy()),
        _ => false,
    }
}

pub fn available_space(dir: &Path) -> std::io::Result<u64> {
    use std::ptr::null_mut;
    let p = to_long_path(dir);
    let wide: Vec<u16> = p
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller = 0u64;
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            null_mut(),
            null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(free_to_caller)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_same_path() {
        let err = ensure_not_inside(Path::new(r"D:\a\b"), Path::new(r"D:\a\b")).unwrap_err();
        assert_eq!(err, PathError::SamePath);
    }

    #[test]
    fn rejects_inside_source() {
        let err = ensure_not_inside(Path::new(r"D:\a\b\c"), Path::new(r"D:\a")).unwrap_err();
        assert_eq!(err, PathError::InsideSource);
    }

    #[test]
    fn accepts_sibling() {
        assert!(ensure_not_inside(Path::new(r"D:\a\b"), Path::new(r"D:\a\c")).is_ok());
    }

    #[test]
    fn case_insensitive_compare() {
        assert!(ensure_not_inside(Path::new(r"D:\A\B"), Path::new(r"d:\a")).is_err());
    }

    #[test]
    fn long_path_prefix() {
        let p = to_long_path(Path::new(r"D:\Photos"));
        assert_eq!(p.to_string_lossy(), r"\\?\D:\Photos");
    }

    #[test]
    fn long_path_untouched() {
        let p = to_long_path(Path::new(r"\\?\D:\Photos"));
        assert_eq!(p.to_string_lossy(), r"\\?\D:\Photos");
    }

    #[test]
    fn unc_long_path() {
        let p = to_long_path(Path::new(r"\\server\share\x"));
        assert_eq!(p.to_string_lossy(), r"\\?\UNC\server\share\x");
    }

    #[test]
    fn long_path_normalizes_slashes() {
        let p = to_long_path(Path::new(r"D:\a/b/c.txt"));
        assert_eq!(p.to_string_lossy(), r"\\?\D:\a\b\c.txt");
    }

    #[test]
    fn same_drive_detects() {
        assert!(same_drive(Path::new(r"D:\a"), Path::new(r"D:\b")));
        assert!(!same_drive(Path::new(r"D:\a"), Path::new(r"E:\b")));
    }
}
