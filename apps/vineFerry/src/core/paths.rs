use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    SamePath,
    InsideSource,
    RelativePath,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::SamePath => write!(f, "源与目标路径相同"),
            PathError::InsideSource => write!(f, "目标位于源目录内部"),
            PathError::RelativePath => write!(f, "不支持相对路径"),
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
        (Some(ca), Some(cb)) => {
            ca.as_os_str().to_string_lossy().eq_ignore_ascii_case(&cb.as_os_str().to_string_lossy())
        }
        _ => false,
    }
}

pub const PORTABLE_MARKER: &str = "vineferry.portable";

pub fn is_portable() -> bool {
    exe_marker().is_some_and(|m| m.exists())
}

fn exe_marker() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(PORTABLE_MARKER)))
}

pub fn data_dir() -> PathBuf {
    resolve_data_dir(
        is_portable(),
        exe_marker().and_then(|m| m.parent().map(Path::to_path_buf)),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    )
}

fn resolve_data_dir(
    portable: bool,
    exe_dir: Option<PathBuf>,
    localappdata: Option<PathBuf>,
) -> PathBuf {
    if portable
        && let Some(dir) = exe_dir
    {
        let d = dir.join("portable_data");
        let _ = std::fs::create_dir_all(&d);
        return d;
    }
    localappdata
        .unwrap_or_else(std::env::temp_dir)
        .join("vineFerry")
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

    #[test]
    fn installed_data_dir_under_localappdata() {
        let la = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let d = resolve_data_dir(false, Some(PathBuf::from(r"C:\app")), la.clone());
        let expect = la
            .unwrap_or_else(std::env::temp_dir)
            .join("vineFerry");
        assert_eq!(d, expect);
    }

    #[test]
    fn portable_data_dir_next_to_exe() {
        let d = resolve_data_dir(true, Some(PathBuf::from(r"C:\app")), None);
        assert_eq!(d, PathBuf::from(r"C:\app\portable_data"));
    }

    #[test]
    fn portable_fallback_when_no_exe_dir() {
        let d = resolve_data_dir(true, None, Some(PathBuf::from(r"C:\Users\x\AppData\Local")));
        assert!(d.to_string_lossy().contains("vineFerry"));
    }

    #[test]
    fn default_db_path_under_data_dir() {
        let p = crate::core::storage::default_db_path();
        assert!(p.ends_with("history.db"));
        assert!(p.parent().unwrap().ends_with("vineFerry") || p.to_string_lossy().contains("portable_data"));
    }
}
