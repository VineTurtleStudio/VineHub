use std::path::Path;

use crate::core::hash::{hash_file, HashAlgo};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ManifestEntry {
    pub hash: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyStatus {
    Ok,
    Missing,
    Mismatch,
    ReadError,
}

impl VerifyStatus {
    pub fn label(self) -> &'static str {
        match self {
            VerifyStatus::Ok => "一致",
            VerifyStatus::Missing => "缺失",
            VerifyStatus::Mismatch => "不一致",
            VerifyStatus::ReadError => "读取失败",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyItem {
    pub path: String,
    pub status: VerifyStatus,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub message: String,
}

pub fn parse_manifest(text: &str) -> Vec<ManifestEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (hash_part, path_part) = match line.find(" *") {
            Some(pos) => (&line[..pos], &line[pos + 2..]),
            None => match line.find("  ") {
                Some(pos) => (&line[..pos], &line[pos + 2..]),
                None => match line.find(char::is_whitespace) {
                    Some(pos) => (&line[..pos], line[pos..].trim()),
                    None => continue,
                },
            },
        };
        if hash_part.is_empty() || path_part.is_empty() {
            continue;
        }
        out.push(ManifestEntry {
            hash: hash_part.to_ascii_lowercase(),
            path: path_part.replace('\\', "/"),
        });
    }
    out
}

pub fn manifest_line(entry: &ManifestEntry) -> String {
    format!("{} *{}", entry.hash, entry.path)
}

pub fn write_manifest(path: &Path, entries: &[ManifestEntry]) -> std::io::Result<()> {
    let mut text = String::new();
    for e in entries {
        text.push_str(&manifest_line(e));
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(crate::core::paths::to_long_path(parent))?;
    }
    std::fs::write(crate::core::paths::to_long_path(path), text)
}

pub fn build_manifest(root: &Path, algo: HashAlgo) -> std::io::Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() || crate::core::paths::is_reparse_point(&meta) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map(|s| s.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if rel.is_empty() {
            continue;
        }
        let hash = hash_file(algo, entry.path())?;
        entries.push(ManifestEntry { hash, path: rel });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

pub fn verify_against_manifest(
    entries: &[ManifestEntry],
    base: &Path,
    algo: HashAlgo,
    mut on_progress: impl FnMut(usize, usize),
) -> Vec<VerifyItem> {
    let total = entries.len();
    let mut out = Vec::with_capacity(total);
    for (i, e) in entries.iter().enumerate() {
        on_progress(i + 1, total);
        let path = base.join(&e.path);
        match std::fs::symlink_metadata(&path) {
            Err(_) => {
                out.push(VerifyItem {
                    path: e.path.clone(),
                    status: VerifyStatus::Missing,
                    expected: Some(e.hash.clone()),
                    actual: None,
                    message: "文件不存在".into(),
                });
                continue;
            }
            Ok(m) if m.is_dir() => {
                out.push(VerifyItem {
                    path: e.path.clone(),
                    status: VerifyStatus::Missing,
                    expected: Some(e.hash.clone()),
                    actual: None,
                    message: "路径是目录".into(),
                });
                continue;
            }
            _ => {}
        }
        match hash_file(algo, &path) {
            Ok(actual) if actual == e.hash => out.push(VerifyItem {
                path: e.path.clone(),
                status: VerifyStatus::Ok,
                expected: Some(e.hash.clone()),
                actual: Some(actual),
                message: "一致".into(),
            }),
            Ok(actual) => out.push(VerifyItem {
                path: e.path.clone(),
                status: VerifyStatus::Mismatch,
                expected: Some(e.hash.clone()),
                actual: Some(actual),
                message: "哈希不一致".into(),
            }),
            Err(err) => out.push(VerifyItem {
                path: e.path.clone(),
                status: VerifyStatus::ReadError,
                expected: Some(e.hash.clone()),
                actual: None,
                message: format!("读取失败：{err}"),
            }),
        }
    }
    out
}

pub fn verify_single(path: &Path, expected: &str, algo: HashAlgo) -> VerifyItem {
    let display = path.display().to_string();
    let expected = expected.to_ascii_lowercase();
    match std::fs::symlink_metadata(path) {
        Err(_) => VerifyItem {
            path: display,
            status: VerifyStatus::Missing,
            expected: Some(expected),
            actual: None,
            message: "文件不存在".into(),
        },
        Ok(m) if m.is_dir() => VerifyItem {
            path: display,
            status: VerifyStatus::Missing,
            expected: Some(expected),
            actual: None,
            message: "路径是目录".into(),
        },
        _ => match hash_file(algo, path) {
            Ok(actual) if actual == expected => VerifyItem {
                path: display,
                status: VerifyStatus::Ok,
                expected: Some(expected),
                actual: Some(actual),
                message: "一致".into(),
            },
            Ok(actual) => VerifyItem {
                path: display,
                status: VerifyStatus::Mismatch,
                expected: Some(expected),
                actual: Some(actual),
                message: "哈希不一致".into(),
            },
            Err(err) => VerifyItem {
                path: display,
                status: VerifyStatus::ReadError,
                expected: Some(expected),
                actual: None,
                message: format!("读取失败：{err}"),
            },
        },
    }
}

pub fn summarize(items: &[VerifyItem]) -> (u32, u32, u32, u32) {
    let mut ok = 0;
    let mut missing = 0;
    let mut mismatch = 0;
    let mut read_error = 0;
    for it in items {
        match it.status {
            VerifyStatus::Ok => ok += 1,
            VerifyStatus::Missing => missing += 1,
            VerifyStatus::Mismatch => mismatch += 1,
            VerifyStatus::ReadError => read_error += 1,
        }
    }
    (ok, missing, mismatch, read_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_sha256sum_and_sfv() {
        let text = "# comment\n\
            ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad *a.txt\n\
            CBF43926  b/c.txt\n\
            900150983cd24fb0d6963f7d28e17f72 d.txt\n";
        let entries = parse_manifest(text);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[1].hash, "cbf43926");
        assert_eq!(entries[1].path, "b/c.txt");
        assert_eq!(entries[2].path, "d.txt");
    }

    #[test]
    fn manifest_roundtrip_and_verify() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("sub")).unwrap();
        fs::write(t.path().join("a.txt"), b"hello").unwrap();
        fs::write(t.path().join("sub/b.txt"), b"world").unwrap();
        let entries = build_manifest(t.path(), HashAlgo::Sha256).unwrap();
        assert_eq!(entries.len(), 2);
        let mpath = t.path().join("list.sha256");
        write_manifest(&mpath, &entries).unwrap();
        let parsed = parse_manifest(&std::fs::read_to_string(&mpath).unwrap());
        assert_eq!(parsed.len(), 2);
        let items = verify_against_manifest(&parsed, t.path(), HashAlgo::Sha256, |_, _| {});
        assert_eq!(summarize(&items), (2, 0, 0, 0));
    }

    #[test]
    fn verify_detects_tamper_and_missing() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), b"hello").unwrap();
        fs::write(t.path().join("b.txt"), b"world").unwrap();
        let entries = build_manifest(t.path(), HashAlgo::Sha256).unwrap();
        fs::write(t.path().join("a.txt"), b"tampered").unwrap();
        fs::remove_file(t.path().join("b.txt")).unwrap();
        let items = verify_against_manifest(&entries, t.path(), HashAlgo::Sha256, |_, _| {});
        let (ok, missing, mismatch, read_error) = summarize(&items);
        assert_eq!((ok, missing, mismatch, read_error), (0, 1, 1, 0));
    }

    #[test]
    fn verify_single_ok_and_mismatch() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.txt");
        fs::write(&f, b"hello").unwrap();
        let hash = hash_file(HashAlgo::Sha256, &f).unwrap();
        let ok = verify_single(&f, &hash, HashAlgo::Sha256);
        assert_eq!(ok.status, VerifyStatus::Ok);
        let bad = verify_single(&f, "deadbeef", HashAlgo::Sha256);
        assert_eq!(bad.status, VerifyStatus::Mismatch);
        let missing = verify_single(&t.path().join("nope"), &hash, HashAlgo::Sha256);
        assert_eq!(missing.status, VerifyStatus::Missing);
    }

    #[test]
    fn verify_single_case_insensitive_hash() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.txt");
        fs::write(&f, b"hello").unwrap();
        let hash = hash_file(HashAlgo::Sha256, &f).unwrap().to_uppercase();
        let ok = verify_single(&f, &hash, HashAlgo::Sha256);
        assert_eq!(ok.status, VerifyStatus::Ok);
    }
}
