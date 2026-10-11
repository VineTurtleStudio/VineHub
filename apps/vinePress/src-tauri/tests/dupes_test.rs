use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use vine_press_lib::dupes::find_duplicates;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("vinepress_dupes_test").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &PathBuf, rel: &str, content: &[u8]) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

fn no_progress(_: &str, _: usize, _: usize, _: &str) {}

#[test]
fn finds_duplicates_by_content_and_ignores_size_only_twins() {
    let dir = fixture("basic");
    write(&dir, "a.txt", b"hello world");
    write(&dir, "b/copy.txt", b"hello world"); // 内容相同（不同目录）
    write(&dir, "d1.bin", b"same12"); // 同大小不同内容 → 不算重复
    write(&dir, "d2.bin", b"same13");
    write(&dir, "single.txt", b"unique content here");
    write(&dir, "empty1", b"");
    write(&dir, "empty2", b"");

    let cancel = AtomicBool::new(false);
    let mut phases: Vec<String> = Vec::new();
    let groups = find_duplicates(&dir, &cancel, |phase, _i, _t, _rel| {
        phases.push(phase.to_string())
    });

    assert_eq!(groups.len(), 1, "只有 hello world 一组：{:?}", groups);
    let g = &groups[0];
    assert_eq!(g.size, 11);
    assert_eq!(g.files, ["a.txt", "b/copy.txt"]); // 组内按字典序，保留首个确定
    assert_eq!(g.hash.len(), 64); // SHA-256 hex
    assert!(phases.contains(&"scan".to_string()));
    assert!(phases.contains(&"hash".to_string()));

    // 取消前置位 → 立即返回空结果
    let cancelled = AtomicBool::new(true);
    let out = find_duplicates(&dir, &cancelled, no_progress);
    assert!(out.is_empty());
}

#[test]
fn skips_system_files_and_reports_hashes() {
    let dir = fixture("system");
    write(&dir, "x.dat", b"payload");
    write(&dir, "y.dat", b"payload");
    write(&dir, "desktop.ini", b"payload");
    write(&dir, "Thumbs.db", b"payload");

    let cancel = AtomicBool::new(false);
    let groups = find_duplicates(&dir, &cancel, no_progress);
    // 系统文件不参与 → x/y 仍成组，且组内只有这两个
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].files, ["x.dat", "y.dat"]);
}

#[test]
fn hash_progress_reports_totals() {
    let dir = fixture("progress");
    for name in ["p1", "p2", "p3", "p4"] {
        write(&dir, name, b"same payload");
    }
    write(&dir, "other", b"different");

    let cancel = AtomicBool::new(false);
    let mut seen: Vec<(usize, usize, String)> = Vec::new();
    let groups = find_duplicates(&dir, &cancel, |phase, index, total, rel| {
        if phase == "hash" {
            seen.push((index, total, rel.to_string()));
        }
    });
    // 4 个同大小文件全部是候选 → hash 阶段 total=4，index 1..=4
    assert_eq!(seen.len(), 4);
    assert!(seen.iter().all(|(_, t, _)| *t == 4));
    assert_eq!(seen[0].0, 1);
    assert_eq!(seen[3].0, 4);
    // 4 份相同 → 一组 4 个文件
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].files.len(), 4);
}
