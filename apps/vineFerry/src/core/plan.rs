use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::core::conflict::{rename_dest, ConflictPolicy};
use crate::core::paths::{ensure_not_inside, is_reparse_point};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Add,
    Update,
    Skip,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlanItem {
    pub rel: String,
    pub src: PathBuf,
    pub dest: PathBuf,
    pub size: u64,
    pub kind: ChangeKind,
    /// 目录项：执行时只创建目录（size 恒为 0），空文件夹也能被复制/移动。
    pub is_dir: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Plan {
    pub src_root: PathBuf,
    pub dest_root: PathBuf,
    pub items: Vec<PlanItem>,
    pub total_bytes: u64,
    pub transfer_bytes: u64,
    pub skipped_bytes: u64,
    pub conflicts: u32,
}

#[derive(Debug)]
pub enum PlanError {
    SourceMissing(PathBuf),
    InsideSource(crate::core::paths::PathError),
    UnresolvedConflicts(u32),
    Io(std::io::Error),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::SourceMissing(p) => write!(f, "源不存在：{}", p.display()),
            PlanError::InsideSource(e) => write!(f, "{e}"),
            PlanError::UnresolvedConflicts(n) => {
                write!(f, "存在 {n} 个同名冲突，需要先选择冲突策略")
            }
            PlanError::Io(e) => write!(f, "IO 错误：{e}"),
        }
    }
}

impl std::error::Error for PlanError {}

impl From<std::io::Error> for PlanError {
    fn from(e: std::io::Error) -> Self {
        PlanError::Io(e)
    }
}

pub fn build_plan(src: &Path, dest_dir: &Path, policy: ConflictPolicy) -> Result<Plan, PlanError> {
    ensure_not_inside(dest_dir, src).map_err(PlanError::InsideSource)?;
    if !src.exists() {
        return Err(PlanError::SourceMissing(src.to_path_buf()));
    }

    let mut items = Vec::new();
    let mut conflicts = 0u32;

    let mut push = |rel: String, src_path: PathBuf, dest_path: PathBuf, size: u64, is_dir: bool| {
        if is_dir {
            // 目录按 Explorer 合并语义处理：存在即 Update，不计冲突、不改名。
            let kind = if dest_path.exists() {
                ChangeKind::Update
            } else {
                ChangeKind::Add
            };
            items.push(PlanItem {
                rel,
                src: src_path,
                dest: dest_path,
                size: 0,
                kind,
                is_dir: true,
            });
            return;
        }
        let kind = if dest_path.exists() {
            conflicts += 1;
            match policy {
                ConflictPolicy::Ask => ChangeKind::Skip,
                ConflictPolicy::Skip => ChangeKind::Skip,
                ConflictPolicy::Overwrite | ConflictPolicy::Merge => ChangeKind::Update,
                ConflictPolicy::Rename => ChangeKind::Add,
            }
        } else {
            ChangeKind::Add
        };
        let dest = if kind == ChangeKind::Add && dest_path.exists() {
            dest_dir.join(rename_dest(dest_dir, &rel))
        } else {
            dest_path
        };
        items.push(PlanItem {
            rel,
            src: src_path,
            dest,
            size,
            kind,
            is_dir: false,
        });
    };

    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_file() {
        let name = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        push(
            name.clone(),
            src.to_path_buf(),
            dest_dir.join(&name),
            meta.len(),
            false,
        );
    } else {
        // 目录：rel 必须带上文件夹自身名字，否则内容会被打散到目标根部。
        let top = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        // 目标已有同名文件夹时：Rename 策略整树去重（如 "a (2)/…"），其余策略原地合并。
        let mut prefix = top.clone();
        if !top.is_empty()
            && dest_dir.join(&top).exists()
            && policy == ConflictPolicy::Rename
        {
            prefix = rename_dest(dest_dir, &top).to_string_lossy().replace('\\', "/");
        }
        let full = |rel_inner: &str| -> String {
            if prefix.is_empty() {
                rel_inner.to_string()
            } else if rel_inner.is_empty() {
                prefix.clone()
            } else {
                format!("{prefix}/{rel_inner}")
            }
        };
        // 顶层目录项：空文件夹也能落地。
        if !prefix.is_empty() {
            push(
                full(""),
                src.to_path_buf(),
                dest_dir.join(&prefix),
                0,
                true,
            );
        }
        for entry in WalkDir::new(src)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                std::fs::symlink_metadata(e.path())
                    .map(|m| !is_reparse_point(&m))
                    .unwrap_or(false)
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let em = match std::fs::symlink_metadata(entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !em.is_file() && !em.is_dir() {
                continue;
            }
            let rel_inner = entry
                .path()
                .strip_prefix(src)
                .map(|s| s.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel_inner.is_empty() {
                // 顶层目录已在上方单独推送
                continue;
            }
            let rel = full(&rel_inner);
            let dest_path = dest_dir.join(&rel);
            push(
                rel,
                entry.path().to_path_buf(),
                dest_path,
                if em.is_dir() { 0 } else { em.len() },
                em.is_dir(),
            );
        }
    }

    if policy == ConflictPolicy::Ask && conflicts > 0 {
        return Err(PlanError::UnresolvedConflicts(conflicts));
    }

    let mut total_bytes = 0u64;
    let mut transfer_bytes = 0u64;
    let mut skipped_bytes = 0u64;
    for it in &items {
        total_bytes += it.size;
        if it.kind == ChangeKind::Skip {
            skipped_bytes += it.size;
        } else {
            transfer_bytes += it.size;
        }
    }

    Ok(Plan {
        src_root: src.to_path_buf(),
        dest_root: dest_dir.to_path_buf(),
        items,
        total_bytes,
        transfer_bytes,
        skipped_bytes,
        conflicts,
    })
}

pub fn concat(plans: Vec<Plan>) -> Plan {
    let mut out = Plan::default();
    for p in plans {
        out.src_root = p.src_root;
        out.dest_root = p.dest_root;
        out.conflicts += p.conflicts;
        out.items.extend(p.items);
    }
    out.total_bytes = out.items.iter().map(|i| i.size).sum();
    out.transfer_bytes = out
        .items
        .iter()
        .filter(|i| i.kind != ChangeKind::Skip)
        .map(|i| i.size)
        .sum();
    out.skipped_bytes = out.total_bytes - out.transfer_bytes;
    out
}

pub fn available_space(dir: &Path) -> std::io::Result<u64> {
    use std::ptr::null_mut;
    let p = crate::core::paths::to_long_path(dir);
    let wide: Vec<u16> = p.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
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
    use std::fs;

    fn seed(dir: &Path) {
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("sub/b.txt"), b"world!!").unwrap();
    }

    #[test]
    fn plan_adds_files() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let dest = t.path().join("dst");
        fs::create_dir_all(&dest).unwrap();
        let plan = build_plan(&t.path().join("src"), &dest, ConflictPolicy::Ask).unwrap();
        // src 目录 + sub 目录 + a.txt + sub/b.txt
        assert_eq!(plan.items.len(), 4);
        assert!(plan.items.iter().all(|i| i.kind == ChangeKind::Add));
        assert_eq!(plan.transfer_bytes, 5 + 7);
    }

    #[test]
    fn plan_preserves_folder_structure() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let dest = t.path().join("dst");
        fs::create_dir_all(&dest).unwrap();
        let plan = build_plan(&t.path().join("src"), &dest, ConflictPolicy::Ask).unwrap();
        let top = plan.items.iter().find(|i| i.rel == "src").unwrap();
        assert!(top.is_dir);
        assert_eq!(top.dest, dest.join("src"));
        let f = plan.items.iter().find(|i| i.rel == "src/a.txt").unwrap();
        assert_eq!(f.dest, dest.join("src/a.txt"));
        let s = plan
            .items
            .iter()
            .find(|i| i.rel == "src/sub/b.txt")
            .unwrap();
        assert_eq!(s.dest, dest.join("src/sub/b.txt"));
    }

    #[test]
    fn plan_empty_folder_still_created() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("empty");
        fs::create_dir_all(&src).unwrap();
        let dest = t.path().join("dst");
        fs::create_dir_all(&dest).unwrap();
        let plan = build_plan(&src, &dest, ConflictPolicy::Ask).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert!(plan.items[0].is_dir);
        assert_eq!(plan.items[0].dest, dest.join("empty"));
    }

    #[test]
    fn plan_rename_duplicates_whole_folder() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let dest = t.path().join("dst");
        fs::create_dir_all(dest.join("src")).unwrap();
        fs::write(dest.join("src/a.txt"), b"old").unwrap();
        let plan = build_plan(&t.path().join("src"), &dest, ConflictPolicy::Rename).unwrap();
        assert_eq!(plan.conflicts, 0);
        assert!(
            plan.items.iter().all(|i| i.rel.starts_with("src (2)/") || i.rel == "src (2)"),
            "rel 应整体去重前缀：{:?}",
            plan.items.iter().map(|i| &i.rel).collect::<Vec<_>>()
        );
        let f = plan.items.iter().find(|i| i.rel == "src (2)/a.txt").unwrap();
        assert_eq!(f.dest, dest.join("src (2)/a.txt"));
    }

    #[test]
    fn plan_overwrite_marks_update() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let dest = t.path().join("dst");
        fs::create_dir_all(dest.join("src")).unwrap();
        fs::write(dest.join("src/a.txt"), b"old").unwrap();
        let plan = build_plan(&t.path().join("src"), &dest, ConflictPolicy::Overwrite).unwrap();
        let item = plan.items.iter().find(|i| i.rel.ends_with("a.txt")).unwrap();
        assert_eq!(item.kind, ChangeKind::Update);
    }

    #[test]
    fn plan_skip_marks_skip() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let dest = t.path().join("dst");
        fs::create_dir_all(dest.join("src")).unwrap();
        fs::write(dest.join("src/a.txt"), b"old").unwrap();
        let plan = build_plan(&t.path().join("src"), &dest, ConflictPolicy::Skip).unwrap();
        let item = plan.items.iter().find(|i| i.rel.ends_with("a.txt")).unwrap();
        assert_eq!(item.kind, ChangeKind::Skip);
        assert!(plan.skipped_bytes >= 5);
    }

    #[test]
    fn plan_rename_dedupes() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.txt");
        fs::write(&src, b"new").unwrap();
        let dest = t.path().join("dst");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("a.txt"), b"old").unwrap();
        let plan = build_plan(&src, &dest, ConflictPolicy::Rename).unwrap();
        let item = &plan.items[0];
        assert_eq!(item.dest.file_name().unwrap().to_string_lossy(), "a (2).txt");
    }

    #[test]
    fn ask_with_conflict_errors() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let dest = t.path().join("dst");
        fs::create_dir_all(dest.join("src")).unwrap();
        fs::write(dest.join("src/a.txt"), b"old").unwrap();
        let err = build_plan(&t.path().join("src"), &dest, ConflictPolicy::Ask).unwrap_err();
        assert!(matches!(err, PlanError::UnresolvedConflicts(1)));
    }

    #[test]
    fn rejects_dest_inside_source() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path().join("src").as_path());
        let err = build_plan(
            &t.path().join("src"),
            &t.path().join("src/inner"),
            ConflictPolicy::Ask,
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::InsideSource(_)));
    }

    #[test]
    fn single_file_plan() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("one.bin");
        fs::write(&src, b"1234").unwrap();
        let dest = t.path().join("dst");
        fs::create_dir_all(&dest).unwrap();
        let plan = build_plan(&src, &dest, ConflictPolicy::Ask).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].rel, "one.bin");
    }

    #[test]
    fn missing_source_errors() {
        let t = tempfile::tempdir().unwrap();
        let err = build_plan(
            &t.path().join("nope"),
            t.path(),
            ConflictPolicy::Ask,
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::SourceMissing(_)));
    }

    #[test]
    fn concat_merges_and_sums() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        fs::write(&a, b"12345").unwrap();
        fs::write(&b, b"67").unwrap();
        let dest = t.path().join("dst");
        fs::create_dir_all(&dest).unwrap();
        let p1 = build_plan(&a, &dest, ConflictPolicy::Ask).unwrap();
        let p2 = build_plan(&b, &dest, ConflictPolicy::Ask).unwrap();
        let merged = concat(vec![p1, p2]);
        assert_eq!(merged.items.len(), 2);
        assert_eq!(merged.transfer_bytes, 7);
        assert_eq!(merged.total_bytes, 7);
    }
}
