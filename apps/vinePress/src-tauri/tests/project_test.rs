use std::fs;
use std::path::PathBuf;
use vine_press_lib::project::{ArchiveSettings, Project};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("vinepress_project").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn sidecar(path: &PathBuf, ext: &str) -> PathBuf {
    let mut s = path.to_string_lossy().to_string();
    s.push('.');
    s.push_str(ext);
    PathBuf::from(s)
}

#[test]
fn save_is_atomic_and_load_falls_back_to_bak() {
    let dir = temp_dir("atomic");
    let path = dir.join("vinepress-project.json");

    let mut p = Project::new("第一版", "D:/root", "D:/out");
    p.save(&path).expect("首次保存失败");
    assert!(path.exists());
    assert!(!sidecar(&path, "tmp").exists(), "保存后不应残留 .tmp");

    p.name = "第二版".into();
    p.save(&path).expect("二次保存失败");
    assert!(!sidecar(&path, "tmp").exists(), "保存后不应残留 .tmp");
    let bak = sidecar(&path, "bak");
    assert!(bak.exists(), "二次保存应留下 .bak");
    let bak_text = fs::read_to_string(&bak).unwrap();
    assert!(bak_text.contains("第一版"), ".bak 应是上一版内容");

    // 正常加载读主文件
    let loaded = Project::load(&path).unwrap();
    assert_eq!(loaded.name, "第二版");

    // 主文件损坏 -> 回退 .bak
    fs::write(&path, "{ 这不是 json").unwrap();
    let fallback = Project::load(&path).expect("应回退到 .bak");
    assert_eq!(fallback.name, "第一版");

    // 主文件与 .bak 都坏 -> 报错
    fs::write(&bak, "也坏掉").unwrap();
    assert!(Project::load(&path).is_err());
}

#[test]
fn volume_mib_zero_is_preserved() {
    let dir = temp_dir("volume");
    let path = dir.join("vinepress-project.json");

    let mut p = Project::new("t", "D:/root", "D:/out");
    p.archive.volume_mib = 0;
    p.save(&path).unwrap();
    let loaded = Project::load(&path).unwrap();
    assert_eq!(loaded.archive.volume_mib, 0, "0 表示不分卷，不能被改写");

    // 缺字段 -> 默认 900
    let json = r#"{
      "version": 1,
      "name": "缺省",
      "root": "D:/root",
      "output_root": "D:/out",
      "password_profiles": [],
      "archive": { "format": "rar", "comment": true, "encrypt_names": true,
                   "overwrite": false, "write_notes": true },
      "entries": []
    }"#;
    fs::write(&path, json).unwrap();
    let loaded = Project::load(&path).unwrap();
    assert_eq!(loaded.archive.volume_mib, 900);
    assert_eq!(loaded.archive.format, "rar");
}

#[test]
fn default_settings_keep_volume_900() {
    let s = ArchiveSettings::default();
    assert_eq!(s.volume_mib, 900);
    assert_eq!(s.format, "rar");
}

#[test]
fn overrides_default_when_missing_from_old_project() {
    let dir = temp_dir("overrides");
    let path = dir.join("vinepress-project.json");
    let json = r#"{
      "version": 1,
      "name": "旧项目",
      "root": "D:/root",
      "output_root": "D:/out",
      "password_profiles": [],
      "archive": { "format": "rar", "comment": true, "encrypt_names": true,
                   "overwrite": false, "write_notes": true, "volume_mib": 900 },
      "entries": []
    }"#;
    fs::write(&path, json).unwrap();
    let p = Project::load(&path).unwrap();
    assert!(p.overrides.is_empty(), "旧项目缺 overrides 字段时应为空");

    let mut ov = p.overrides;
    ov.set_unit("分类A");
    ov.set_unit("分类B");
    ov.set_container("分类B");
    assert_eq!(ov.force_units, vec!["分类A".to_string()]);
    assert_eq!(ov.force_containers, vec!["分类B".to_string()]);
}

#[test]
fn rename_entry_paths_rewrites_self_and_descendants() {
    use vine_press_lib::project::{rename_entry_paths, Entry};
    use vine_press_lib::scan::UnitStats;

    let mk = |rel: &str| Entry {
        rel_path: rel.to_string(),
        name: rel.rsplit('/').next().unwrap().to_string(),
        kind: "album".into(),
        work: "作品".into(),
        category: "分类".into(),
        title: format!("原标题 {}", rel),
        artist: String::new(),
        date: String::new(),
        label: String::new(),
        catalog: String::new(),
        publisher: String::new(),
        rank: String::new(),
        notes: String::new(),
        tags: Vec::new(),
        tracklist: Vec::new(),
        password_profile: None,
        selected: true,
        status: "pending".into(),
        has_notes: false,
        needs_review: false,
        stats: UnitStats::default(),
    };
    let mut entries = vec![
        mk("分类/单位A"),
        mk("分类/单位A/内部子目录"),
        mk("分类/单位A兄弟"),
        mk("分类/单位B"),
    ];
    let titles: Vec<String> = entries.iter().map(|e| e.title.clone()).collect();

    rename_entry_paths(&mut entries, "分类/单位A", "分类/新名字");

    assert_eq!(entries[0].rel_path, "分类/新名字");
    assert_eq!(entries[0].name, "新名字", "name 应取新路径末段");
    assert_eq!(entries[1].rel_path, "分类/新名字/内部子目录", "子孙条目前缀同步");
    assert_eq!(entries[1].name, "内部子目录", "子孙 name 不变");
    assert_eq!(entries[2].rel_path, "分类/单位A兄弟", "同前缀但非后代的条目不能被误伤");
    assert_eq!(entries[3].rel_path, "分类/单位B");
    let titles2: Vec<String> = entries.iter().map(|e| e.title.clone()).collect();
    assert_eq!(titles, titles2, "不改写用户编辑过的标题");

    // 无效调用应是无操作
    let before: Vec<String> = entries.iter().map(|e| e.rel_path.clone()).collect();
    rename_entry_paths(&mut entries, "", "x");
    rename_entry_paths(&mut entries, "分类/新名字", "分类/新名字");
    assert_eq!(
        before,
        entries.iter().map(|e| e.rel_path.clone()).collect::<Vec<_>>()
    );
}
