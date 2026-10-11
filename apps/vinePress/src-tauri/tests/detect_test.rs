use std::fs;
use std::path::PathBuf;
use vine_press_lib::detect::detect;
use vine_press_lib::project::Overrides;
use vine_press_lib::scan::scan_tree;

/// ```
/// root/
/// ├─ 容器A/ 单位1/a.jpg, 单位2/b.jpg
/// ├─ 容器B/ 子/cover.jpg, 子/单位3/c.jpg
/// └─ 双层/ 外层/inner.jpg, 外层/子盘/d.jpg
/// ```
/// 每个测试用独立目录，避免并行互踩。
fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("vinepress_detect_ov")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("容器A").join("单位1")).unwrap();
    fs::create_dir_all(d.join("容器A").join("单位2")).unwrap();
    fs::create_dir_all(d.join("容器B").join("子").join("单位3")).unwrap();
    fs::create_dir_all(d.join("双层").join("外层").join("子盘")).unwrap();
    fs::write(d.join("容器A/单位1/a.jpg"), b"jpg").unwrap();
    fs::write(d.join("容器A/单位2/b.jpg"), b"jpg").unwrap();
    fs::write(d.join("容器B/子/cover.jpg"), b"jpg").unwrap();
    fs::write(d.join("容器B/子/单位3/c.jpg"), b"jpg").unwrap();
    fs::write(d.join("双层/外层/inner.jpg"), b"jpg").unwrap();
    fs::write(d.join("双层/外层/子盘/d.jpg"), b"jpg").unwrap();
    d
}

fn detect_units(name: &str, ov: &Overrides) -> Vec<String> {
    let root = fixture(name);
    let tree = scan_tree(&root).expect("扫描失败");
    let mut v: Vec<String> = detect(&tree, ov)
        .units
        .iter()
        .map(|u| u.rel_path.clone())
        .collect();
    v.sort();
    v
}

#[test]
fn default_units_without_overrides() {
    let units = detect_units("default", &Overrides::default());
    assert_eq!(
        units,
        vec![
            "双层/外层".to_string(),
            "容器A/单位1".to_string(),
            "容器A/单位2".to_string(),
            "容器B/子".to_string(),
        ]
    );
}

#[test]
fn force_unit_merges_children_into_one_unit() {
    let mut ov = Overrides::default();
    ov.set_unit("容器A");
    let units = detect_units("merge", &ov);
    assert!(
        units.contains(&"容器A".to_string()),
        "容器A 应整体成为一个单位: {:?}",
        units
    );
    assert!(
        !units.iter().any(|u| u.starts_with("容器A/")),
        "子单位应被一并吸收: {:?}",
        units
    );
    assert!(
        units.contains(&"容器B/子".to_string()),
        "其它分支不受影响: {:?}",
        units
    );
    assert!(
        units.contains(&"双层/外层".to_string()),
        "其它分支不受影响: {:?}",
        units
    );
}

#[test]
fn force_container_drills_into_would_be_unit() {
    let mut ov = Overrides::default();
    ov.set_container("双层/外层");
    let units = detect_units("drill", &ov);
    assert!(
        units.contains(&"双层/外层/子盘".to_string()),
        "强制为容器应下钻: {:?}",
        units
    );
    assert!(
        !units.contains(&"双层/外层".to_string()),
        "外层本身不再是单位: {:?}",
        units
    );
}

#[test]
fn deep_override_makes_ancestors_recurse() {
    let mut ov = Overrides::default();
    ov.set_unit("容器B/子/单位3");
    let units = detect_units("deep", &ov);
    // 默认 容器B/子 因「含内容文件」整体成单位，深层覆盖必须让祖先一路下钻
    assert!(
        units.contains(&"容器B/子/单位3".to_string()),
        "深层强制单位应生效: {:?}",
        units
    );
    assert!(
        !units.contains(&"容器B/子".to_string()),
        "祖先不再整体成单位: {:?}",
        units
    );
}

#[test]
fn clearing_override_restores_auto_detection() {
    let mut ov = Overrides::default();
    ov.set_unit("容器A");
    let with_override = detect_units("clear", &ov);
    assert!(with_override.contains(&"容器A".to_string()));

    ov.clear("容器A");
    assert!(ov.is_empty());
    assert_eq!(detect_units("clear2", &ov), detect_units("clear3", &ov));
}

#[test]
fn unit_and_container_override_are_mutually_exclusive() {
    let mut ov = Overrides::default();
    ov.set_unit("a/b");
    ov.set_container("a/b");
    assert!(ov.force_units.is_empty(), "应从 force_units 移除");
    assert_eq!(ov.force_containers, vec!["a/b".to_string()]);

    ov.set_unit("a/b");
    assert!(ov.force_containers.is_empty(), "应从 force_containers 移除");
    assert_eq!(ov.force_units, vec!["a/b".to_string()]);
}

#[test]
fn has_notes_requires_exact_name_and_root_loose_lists_content() {
    let d = std::env::temp_dir()
        .join("vinepress_detect_ov")
        .join("notes_loose");
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("单位甲")).unwrap();
    fs::write(d.join("单位甲/单位甲.txt"), b"n").unwrap();
    fs::create_dir_all(d.join("单位乙")).unwrap();
    fs::write(d.join("单位乙/readme.txt"), b"r").unwrap();
    fs::create_dir_all(d.join("容器/单位丙")).unwrap();
    fs::write(d.join("容器/单位丙/单位丙.TXT"), b"n").unwrap();
    fs::write(d.join("封面.jpg"), b"img").unwrap();
    fs::write(d.join("散曲.flac"), b"aud").unwrap();
    fs::write(d.join("readme.txt"), b"t").unwrap();
    fs::write(d.join("Thumbs.db"), b"x").unwrap();

    let tree = scan_tree(&d).expect("扫描失败");
    let res = detect(&tree, &Overrides::default());
    let has = |rel: &str| {
        res.units
            .iter()
            .find(|u| u.rel_path == rel)
            .unwrap_or_else(|| panic!("缺少单位 {}", rel))
            .has_notes
    };
    assert!(has("单位甲"), "精确 <单位名>.txt 应算已生成");
    assert!(!has("单位乙"), "readme.txt 不应误判为已生成");
    assert!(has("容器/单位丙"), "大小写应宽容");
    // 根散件只报内容文件（txt/系统垃圾不算）
    assert_eq!(res.root_loose.len(), 2, "root_loose = {:?}", res.root_loose);
    assert!(res.root_loose.contains(&"封面.jpg".to_string()));
    assert!(res.root_loose.contains(&"散曲.flac".to_string()));
    assert!(!res.root_loose.iter().any(|n| n == "Thumbs.db" || n == "readme.txt"));
}
