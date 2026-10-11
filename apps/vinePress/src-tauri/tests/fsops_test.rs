use std::fs;
use std::path::PathBuf;
use vine_press_lib::fsops::{
    copy_items, ensure_under, list_dir, make_directory, move_items, relative_to, rename_path,
};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("vinepress_fsops").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn rejects_paths_outside_root() {
    let root = temp_dir("guard");
    let ok = ensure_under(&root.to_string_lossy(), &format!("{}/子目录", root.display()));
    assert!(ok.is_ok());
    let bad = ensure_under(&root.to_string_lossy(), "C:\\Windows\\System32");
    assert!(bad.is_err(), "根目录之外应被拒绝");
    let dotdot = ensure_under(&root.to_string_lossy(), &format!("{}/../outside", root.display()));
    assert!(dotdot.is_err(), "含 .. 应被拒绝");
}

#[test]
fn lists_sorted_with_dirs_first() {
    let root = temp_dir("list");
    fs::write(root.join("b.txt"), b"2").unwrap();
    fs::write(root.join("a.txt"), b"1").unwrap();
    fs::create_dir(root.join("Z目录")).unwrap();
    fs::write(root.join("Z目录").join("inner.txt"), b"x").unwrap();

    let root_s = root.to_string_lossy().to_string();
    let items = list_dir(&root_s, &root_s, false).unwrap();
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names[0], "Z目录", "目录优先: {:?}", names);
    assert_eq!(names[1], "a.txt");
    assert_eq!(names[2], "b.txt");
    let dir_item = items.iter().find(|i| i.name == "Z目录").unwrap();
    assert!(dir_item.is_dir);

    // 隐藏/系统文件默认过滤
    fs::write(root.join("desktop.ini"), b"").unwrap();
    fs::write(root.join(".hidden"), b"").unwrap();
    let items = list_dir(&root_s, &root_s, false).unwrap();
    assert!(!items.iter().any(|i| i.name == "desktop.ini"));
    assert!(!items.iter().any(|i| i.name == ".hidden"));
    let all = list_dir(&root_s, &root_s, true).unwrap();
    assert!(all.iter().any(|i| i.name == "desktop.ini"));
}

#[test]
fn creates_copies_and_moves_under_root() {
    let root = temp_dir("ops");
    let root_s = root.to_string_lossy().to_string();
    let src = root.join("分类A").join("单位1");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.jpg"), b"jpeg").unwrap();
    fs::create_dir_all(src.join("子")).unwrap();
    fs::write(src.join("子").join("b.txt"), b"txt").unwrap();

    // 新建分类
    make_directory(&root_s, &format!("{}/分类B", root_s)).unwrap();
    assert!(root.join("分类B").is_dir());
    assert!(make_directory(&root_s, &format!("{}/分类B", root_s)).is_err());

    // 复制条目到新分类
    let created = copy_items(&root_s, &[src.to_string_lossy().to_string()], &format!("{}/分类B", root_s)).unwrap();
    assert_eq!(created.len(), 1);
    let copied = root.join("分类B").join("单位1");
    assert!(copied.join("a.jpg").exists(), "应复制文件");
    assert!(copied.join("子").join("b.txt").exists(), "应复制子目录");
    assert!(src.exists(), "源目录不应被动");

    // 再次复制 -> 自动重命名
    let created2 = copy_items(&root_s, &[src.to_string_lossy().to_string()], &format!("{}/分类B", root_s)).unwrap();
    assert_eq!(created2.len(), 1);
    let second = PathBuf::from(&created2[0]);
    assert_ne!(second, copied, "应产生不同路径: {:?}", created2);
    assert!(second.exists());

    // 移动
    let moved = move_items(
        &root_s,
        &[src.to_string_lossy().to_string()],
        &format!("{}/分类B", root_s),
    )
    .unwrap();
    assert_eq!(moved.len(), 1);
    assert!(!src.exists(), "移动后源应消失");
    assert!(PathBuf::from(&moved[0]).exists());

    // 目标在根之外 -> 拒绝
    assert!(copy_items(
        &root_s,
        &[moved[0].clone()],
        "C:\\Windows\\Temp"
    )
    .is_err());
}

#[test]
fn rejects_copy_and_move_into_own_subdir() {
    let root = temp_dir("nested");
    let root_s = root.to_string_lossy().to_string();
    let src = root.join("源目录");
    fs::create_dir_all(src.join("内层")).unwrap();
    fs::write(src.join("内层").join("a.jpg"), b"jpeg").unwrap();

    let src_s = src.to_string_lossy().to_string();
    let dest_s = format!("{}/源目录/内层", root_s);

    // 复制到自身子目录 -> 拒绝（否则无限递归）
    let err = copy_items(&root_s, &[src_s.clone()], &dest_s).unwrap_err();
    assert!(err.contains("自身子目录"), "错误信息应说明原因: {}", err);
    assert!(!root.join("源目录/内层").join("源目录").exists());

    // 目标目录尚未存在时也不能悄悄创建残留
    let dest2 = format!("{}/源目录/新建子目录", root_s);
    assert!(copy_items(&root_s, &[src_s.clone()], &dest2).is_err());
    assert!(!root.join("源目录/新建子目录").exists(), "拒绝时不应留下目录");

    // 移动到自身子目录 -> 拒绝
    assert!(move_items(&root_s, &[src_s.clone()], &dest_s).is_err());
    assert!(src.exists(), "源目录必须完好");

    // 同级目录复制仍然允许
    let ok = copy_items(
        &root_s,
        &[src_s.clone()],
        &format!("{}/兄弟", root_s),
    )
    .unwrap();
    assert_eq!(ok.len(), 1);
    assert!(PathBuf::from(&ok[0]).exists());
}

#[test]
fn validate_rel_rejects_traversal() {
    use vine_press_lib::fsops::validate_rel;

    assert!(validate_rel("分类/单位A").is_ok());
    assert!(validate_rel("单位A").is_ok());
    assert!(validate_rel("").is_err());
    assert!(validate_rel("  ").is_err());
    assert!(validate_rel("../逃逸").is_err());
    assert!(validate_rel("分类/../逃逸").is_err());
    assert!(validate_rel("/绝对路径").is_err());
    assert!(validate_rel("C:/盘符").is_err());
    assert!(validate_rel("a\\b").is_err(), "反斜杠应被拒绝");
    assert!(validate_rel("./当前").is_err());
}

#[test]
fn renames_directories_with_validation() {
    let root = temp_dir("rename");
    let root_s = root.to_string_lossy().to_string();
    let unit = root.join("分类").join("旧单位");
    fs::create_dir_all(unit.join("子")).unwrap();
    fs::write(unit.join("a.jpg"), b"jpeg").unwrap();
    let other = root.join("分类").join("别的单位");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("b.jpg"), b"x").unwrap();

    let unit_s = unit.to_string_lossy().to_string();

    // 正常改名
    let new_abs = rename_path(&root_s, &unit_s, "新单位").expect("改名失败");
    assert!(new_abs.ends_with("新单位"), "返回新路径: {}", new_abs);
    assert!(root.join("分类/新单位").is_dir());
    assert!(root.join("分类/新单位/a.jpg").is_file(), "内容随目录迁移");
    assert!(!unit.exists());

    // 非法名称
    for bad in ["", "  ", "带/斜杠", "带:冒号", "尾随点.", "尾随空格 ", "CON", "con.jpg", ".."] {
        assert!(
            rename_path(&root_s, &root.join("分类").join("新单位").to_string_lossy(), bad).is_err(),
            "应拒绝 {:?}",
            bad
        );
    }

    // 目标已存在
    fs::create_dir_all(root.join("分类").join("已存在")).unwrap();
    assert!(rename_path(&root_s, &root.join("分类/新单位").to_string_lossy(), "已存在").is_err());

    // 不允许改根
    assert!(rename_path(&root_s, &root_s, "换个名字").is_err());

    // 路径在根之外 / 不存在
    assert!(rename_path(&root_s, "C:\\Windows\\System32", "x").is_err());
    assert!(rename_path(&root_s, &format!("{}/不存在", root_s), "x").is_err());
}

#[test]
fn relative_to_preserves_case_and_joins_with_slash() {
    let root = r"D:\Media\库";
    assert_eq!(relative_to(root, r"D:\Media\库\分类\单位A").unwrap(), "分类/单位A");
    assert_eq!(relative_to(root, "D:/Media/库/分类/单位A").unwrap(), "分类/单位A");
    assert_eq!(relative_to(root, r"D:\Media\库").unwrap(), "");
    assert_eq!(relative_to(root, r"D:\MEDIA\库\分类\单位A").unwrap(), "分类/单位A");
    assert!(relative_to(root, r"D:\Media\库2\单位").is_none(), "前缀相同但不同根");
    assert!(relative_to(root, r"D:\Other\x").is_none());
    assert!(relative_to(root, r"单位").is_none());
}

#[test]
fn rename_syncs_note_file() {
    let root = temp_dir("rename_note");
    let root_s = root.to_string_lossy().to_string();
    let unit = root.join("旧单位");
    fs::create_dir_all(&unit).unwrap();
    fs::write(unit.join("旧单位.txt"), b"note").unwrap();
    fs::write(unit.join("readme.txt"), b"r").unwrap();
    fs::write(unit.join("a.jpg"), b"jpg").unwrap();

    rename_path(&root_s, &unit.to_string_lossy(), "新单位").expect("改名失败");
    let new_dir = root.join("新单位");
    assert!(new_dir.join("新单位.txt").is_file(), "受管说明文件应随目录改名");
    assert!(!new_dir.join("旧单位.txt").exists(), "旧说明文件名不应残留");
    assert!(new_dir.join("readme.txt").is_file(), "其他 .txt 不应被动");

    // 目标说明文件已存在 → 不覆盖
    let u2 = root.join("旧B");
    fs::create_dir_all(&u2).unwrap();
    fs::write(u2.join("旧B.txt"), b"n1").unwrap();
    fs::write(u2.join("新B.txt"), b"n2").unwrap();
    rename_path(&root_s, &u2.to_string_lossy(), "新B").expect("改名失败");
    let new2 = root.join("新B");
    assert_eq!(fs::read(new2.join("新B.txt")).unwrap(), b"n2", "已有目标说明文件不被覆盖");
    assert!(new2.join("旧B.txt").is_file(), "冲突时旧说明文件保留");

    // 无说明文件的目录改名不受影响
    let u3 = root.join("无说明");
    fs::create_dir_all(&u3).unwrap();
    rename_path(&root_s, &u3.to_string_lossy(), "有说明?").unwrap_err(); // 非法字符
    rename_path(&root_s, &u3.to_string_lossy(), "改好了").expect("改名失败");
    assert!(root.join("改好了").is_dir());
}
