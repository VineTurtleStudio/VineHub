use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    Skip,
    Overwrite,
    Rename,
}

pub fn dedupe_name(dir: &Path, name: &str) -> String {
    let (stem, ext) = split_name(name);
    for i in 2..10_000 {
        let candidate = if ext.is_empty() {
            format!("{stem} ({i})")
        } else {
            format!("{stem} ({i}).{ext}")
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    if ext.is_empty() {
        format!("{stem} ({})", uuid_like())
    } else {
        format!("{stem} ({}).{ext}", uuid_like())
    }
}

fn split_name(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(pos) if pos > 0 && pos + 1 < name.len() => {
            (name[..pos].to_string(), name[pos + 1..].to_string())
        }
        _ => (name.to_string(), String::new()),
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}")
}

pub fn rename_dest(dest_dir: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string());
    let parent_rel = p.parent().map(|s| s.to_string_lossy().to_string());
    let parent_dir = match &parent_rel {
        Some(sub) if !sub.is_empty() => dest_dir.join(sub),
        _ => dest_dir.to_path_buf(),
    };
    let new_name = dedupe_name(&parent_dir, &name);
    match parent_rel {
        Some(sub) if !sub.is_empty() => {
            PathBuf::from(sub.replace('\\', "/")).join(new_name)
        }
        _ => PathBuf::from(new_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dedupe_appends_counter() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        assert_eq!(dedupe_name(dir.path(), "a.txt"), "a (2).txt");
        fs::write(dir.path().join("a (2).txt"), b"x").unwrap();
        assert_eq!(dedupe_name(dir.path(), "a.txt"), "a (3).txt");
    }

    #[test]
    fn dedupe_extensionless() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("readme"), b"x").unwrap();
        assert_eq!(dedupe_name(dir.path(), "readme"), "readme (2)");
    }

    #[test]
    fn hidden_file_keeps_prefix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), b"x").unwrap();
        assert_eq!(dedupe_name(dir.path(), ".gitignore"), ".gitignore (2)");
    }

    #[test]
    fn rename_dest_keeps_subdir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), b"x").unwrap();
        assert_eq!(
            rename_dest(dir.path(), "sub/b.txt"),
            PathBuf::from("sub/b (2).txt")
        );
    }
}
