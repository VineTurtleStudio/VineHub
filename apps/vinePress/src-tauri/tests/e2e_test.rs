use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use vine_press_lib::detect::detect;
use vine_press_lib::export::{run_export, ExportConfig, ExportProgress};
use vine_press_lib::manifest;
use vine_press_lib::notes::NOTE_FOOTER;
use vine_press_lib::project::{ArchiveSettings, Entry, Overrides, PasswordProfile};
use vine_press_lib::scan::scan_tree;

fn find_rar() -> Option<String> {
    [
        "C:\\Program Files\\WinRAR\\Rar.exe",
        "C:\\Program Files (x86)\\WinRAR\\Rar.exe",
    ]
    .iter()
    .find(|p| std::path::Path::new(p).exists())
    .map(|s| s.to_string())
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("vinepress_e2e").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn build_source(root: &PathBuf) {
    let unit_a = root.join("001 测试作品").join("画集").join("单位A");
    let unit_b = root.join("001 测试作品").join("画集").join("单位B");
    fs::create_dir_all(unit_a.join("本体")).unwrap();
    fs::create_dir_all(&unit_b).unwrap();
    fs::write(unit_a.join("cover.jpg"), b"\xff\xd8\xff\xe0fake-jpeg").unwrap();
    fs::write(unit_a.join("本体").join("p01.jpg"), b"\xff\xd8\xff\xe0fake-jpeg").unwrap();
    fs::write(unit_b.join("readme.md"), b"# hello").unwrap();
}

#[test]
#[ignore = "依赖本机 WinRAR"]
fn end_to_end_scan_notes_export() {
    let rar = match find_rar() {
        Some(r) => r,
        None => {
            eprintln!("skip: 未找到 Rar.exe");
            return;
        }
    };

    let src_root = temp_dir("src");
    let out_root = temp_dir("out");
    build_source(&src_root);

    // 1) 扫描识别
    let tree = scan_tree(&src_root).expect("扫描失败");
    let detected = detect(&tree, &Overrides::default());
    let entries: Vec<Entry> = detected.units.iter().map(Entry::from_unit).collect();
    assert_eq!(entries.len(), 2, "应识别 2 个单位: {:?}", detected.units);
    assert!(
        entries.iter().all(|e| e.category == "画集"),
        "分类应为画集: {:?}",
        entries.iter().map(|e| (&e.category, &e.work)).collect::<Vec<_>>()
    );

    // 2) 密码方案
    let profiles = vec![PasswordProfile {
        id: "sensitive".into(),
        usage: "色情敏感文件".into(),
        passwords: vec!["科学と自然の恋".into()],
    }];
    let mut entries = entries;
    for e in &mut entries {
        e.password_profile = Some("sensitive".into());
        e.selected = true;
    }

    let settings = ArchiveSettings {
        format: "rar".into(),
        volume_mib: 0,
        comment: true,
        encrypt_names: true,
        overwrite: false,
        write_notes: true,
        incremental: false,
    };

    // 3) 导出
    let cfg = ExportConfig {
        rar_path: &rar,
        root: &src_root,
        output_root: &out_root,
        settings: &settings,
        profiles: &profiles,
        entries: &entries,
    };
    let cancel = AtomicBool::new(false);
    let events: std::cell::RefCell<Vec<ExportProgress>> = std::cell::RefCell::new(Vec::new());
    let final_progress = run_export(
        &cfg,
        &|p: &ExportProgress| events.borrow_mut().push(p.clone()),
        &cancel,
    );

    assert_eq!(final_progress.status, "finished");
    assert_eq!(final_progress.created, 2, "应创建 2 个压缩包: {:?}", final_progress);
    assert_eq!(final_progress.failed, 0);

    // 4) 输出结构镜像 + 文件名密码标签（压缩包位于镜像单位目录内）
    for rel in [
        "001 测试作品/画集/单位A/单位A [科学と自然の恋].rar",
        "001 测试作品/画集/单位B/单位B [科学と自然の恋].rar",
    ] {
        let p = out_root.join(rel);
        assert!(p.exists(), "应存在 {}", p.display());
    }

    // 5) 说明文件已写入源目录（UTF-8 BOM + 页脚）
    let note = src_root
        .join("001 测试作品")
        .join("画集")
        .join("单位A")
        .join("单位A.txt");
    let bytes = fs::read(&note).expect("说明文件应存在");
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
    let body = String::from_utf8(bytes[3..].to_vec()).unwrap();
    assert!(body.contains(NOTE_FOOTER), "说明文件应含页脚");

    // 6) 压缩包内容（含说明文件 + 子目录）与注释
    let archive = out_root.join("001 测试作品/画集/单位A/单位A [科学と自然の恋].rar");
    let listing = std::process::Command::new(&rar)
        .arg("l")
        .arg("-p科学と自然の恋")
        .arg(&archive)
        .output()
        .expect("列出失败");
    let text = encoding_rs::GB18030.decode(&listing.stdout).0.into_owned();
    assert!(text.contains("cover.jpg"), "应包含封面: {}", text);
    assert!(text.contains("p01.jpg"), "应包含子目录文件: {}", text);
    assert!(text.contains("单位A.txt"), "应包含说明文件: {}", text);
    assert!(text.contains(NOTE_FOOTER), "注释应等于说明文件正文: {}", text);

    // 7) 未加密列表应被拒绝
    let plain = std::process::Command::new(&rar)
        .arg("l")
        .arg(&archive)
        .output()
        .unwrap();
    let plain_text = encoding_rs::GB18030.decode(&plain.stdout).0.into_owned();
    assert!(!plain_text.contains("cover.jpg"), "未加密不应列出内容");

    // 8) 不覆盖时再次导出应跳过
    let cfg2 = ExportConfig {
        rar_path: &rar,
        root: &src_root,
        output_root: &out_root,
        settings: &settings,
        profiles: &profiles,
        entries: &entries,
    };
    let progress2 = run_export(&cfg2, &|_: &ExportProgress| {}, &cancel);
    assert_eq!(progress2.skipped, 2, "应跳过 2 个: {:?}", progress2);

    assert!(
        events
            .borrow()
            .iter()
            .filter(|e| e.phase == "unit")
            .count()
            >= 4,
        "应有逐单位事件"
    );
}

#[test]
fn end_to_end_zip_export() {
    let src_root = temp_dir("src_zip");
    let out_root = temp_dir("out_zip");
    build_source(&src_root);

    let tree = scan_tree(&src_root).expect("扫描失败");
    let detected = detect(&tree, &Overrides::default());
    let mut entries: Vec<Entry> = detected.units.iter().map(Entry::from_unit).collect();
    assert_eq!(entries.len(), 2);
    for e in &mut entries {
        e.selected = true;
        e.password_profile = Some("sensitive".into());
    }
    let profiles = vec![PasswordProfile {
        id: "sensitive".into(),
        usage: "敏感".into(),
        passwords: vec!["秘密".into()],
    }];
    let settings = ArchiveSettings {
        format: "zip".into(),
        volume_mib: 900,
        comment: true,
        encrypt_names: true,
        overwrite: false,
        write_notes: true,
        incremental: false,
    };

    let cfg = ExportConfig {
        rar_path: "",
        root: &src_root,
        output_root: &out_root,
        settings: &settings,
        profiles: &profiles,
        entries: &entries,
    };
    let cancel = AtomicBool::new(false);
    let phases = std::cell::RefCell::new(Vec::new());
    let final_progress = run_export(&cfg, &|p: &ExportProgress| phases.borrow_mut().push(p.phase.clone()), &cancel);
    assert_eq!(final_progress.status, "finished");
    assert_eq!(final_progress.created, 2, "{:?}", final_progress);
    assert_eq!(final_progress.failed, 0);

    // 导出清单：manifest 阶段（finished 之前）→ 2 个文件，校验全部一致
    let phases = phases.into_inner();
    let mi = phases.iter().position(|x| x == "manifest").expect("应有 manifest 阶段");
    let fi = phases.iter().position(|x| x == "finished").expect("应有 finished");
    assert!(mi < fi, "manifest 应在 finished 之前: {:?}", phases);
    let m = manifest::load(&out_root).expect("导出后应写清单");
    assert_eq!(m.files.len(), 2, "清单应含 2 个压缩包: {:?}", m.files);
    let report = manifest::verify(&out_root, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    assert_eq!(report.ok, 2, "首次校验应全部一致: {:?}", report);
    assert!(report.unlisted.is_empty(), "{:?}", report.unlisted);

    // ZIP 无密码标签、无分卷，位于镜像单位目录内部
    let a = out_root.join("001 测试作品/画集/单位A/单位A.zip");
    let b = out_root.join("001 测试作品/画集/单位B/单位B.zip");
    assert!(a.is_file(), "应存在 {}", a.display());
    assert!(b.is_file(), "应存在 {}", b.display());

    // 说明文件已写入源目录；注释 = 说明正文（GBK）
    let note_bytes = fs::read(
        src_root
            .join("001 测试作品")
            .join("画集")
            .join("单位A")
            .join("单位A.txt"),
    )
    .expect("说明文件应存在");
    assert_eq!(&note_bytes[..3], &[0xEF, 0xBB, 0xBF]);
    let body = String::from_utf8(note_bytes[3..].to_vec()).unwrap();
    assert!(body.contains(NOTE_FOOTER), "说明文件应含页脚");
    // 磁盘文件为 CRLF；注释写入的是同一份渲染文本（LF）
    let body = body.replace("\r\n", "\n");

    let file = fs::File::open(&a).expect("ZIP 应存在");
    let archive = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
    assert!(names.contains(&"cover.jpg".to_string()), "{:?}", names);
    assert!(names.contains(&"单位A.txt".to_string()), "应包含说明文件: {:?}", names);
    let (gbk, _, _) = encoding_rs::GB18030.encode(&body);
    assert_eq!(archive.comment(), &gbk[..], "注释应等于说明正文（GBK）");

    // 不覆盖时再次导出 → 全部跳过（清单 upsert 后仍为 2 条且校验一致）
    let progress2 = run_export(&cfg, &|_: &ExportProgress| {}, &cancel);
    assert_eq!(progress2.skipped, 2, "{:?}", progress2);
    assert_eq!(progress2.created, 0);
    let m2 = manifest::load(&out_root).unwrap();
    assert_eq!(m2.files.len(), 2, "{:?}", m2.files);
    let report2 = manifest::verify(&out_root, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    assert_eq!(report2.ok, 2, "跳过重导后校验仍应一致: {:?}", report2);

    // 篡改磁盘压缩包 → 校验能发现
    fs::write(&a, b"tampered!").unwrap();
    let report3 = manifest::verify(&out_root, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    assert_eq!(
        report3.size_bad + report3.hash_bad,
        1,
        "篡改后应报告 1 个不一致: {:?}",
        report3
    );
}

// ---------------------------------------------------------------- 增量导出（按清单指纹跳过）

fn run_zip_export(
    root: &PathBuf,
    out: &PathBuf,
    settings: &ArchiveSettings,
    entries: &[Entry],
    cancel: &AtomicBool,
    on_event: &dyn Fn(&ExportProgress),
) -> ExportProgress {
    let profiles: Vec<PasswordProfile> = Vec::new();
    let cfg = ExportConfig {
        rar_path: "",
        root,
        output_root: out,
        settings,
        profiles: &profiles,
        entries,
    };
    run_export(&cfg, on_event, cancel)
}

#[test]
fn unit_fingerprint_stability() {
    use vine_press_lib::export::unit_fingerprint;

    let dir = temp_dir("fp_dir");
    fs::write(dir.join("a.txt"), b"hello").unwrap();
    let settings = ArchiveSettings {
        format: "zip".into(),
        volume_mib: 900,
        comment: true,
        encrypt_names: true,
        overwrite: true,
        write_notes: true,
        incremental: true,
    };

    let note_path = dir.join("单位.txt");

    // 相同输入 → 相同指纹
    let f1 = unit_fingerprint(&dir, &settings, None, true, "note-1", Some(&note_path));
    let f2 = unit_fingerprint(&dir, &settings, None, true, "note-1", Some(&note_path));
    assert_eq!(f1, f2);

    // note_included=false 时说明正文不参与指纹
    let f3 = unit_fingerprint(&dir, &settings, None, false, "note-2", None);
    let f4 = unit_fingerprint(&dir, &settings, None, false, "别的正文", None);
    assert_eq!(f3, f4);

    // note_included=true 时正文变化 → 指纹变化
    let f5 = unit_fingerprint(&dir, &settings, None, true, "note-2", Some(&note_path));
    assert_ne!(f1, f5);

    // 受管说明文件落盘不影响指纹（walk 排除 + 正文哈希覆盖）
    fs::write(&note_path, b"bom-note").unwrap();
    let f5b = unit_fingerprint(&dir, &settings, None, true, "note-1", Some(&note_path));
    assert_eq!(f1, f5b, "说明文件写入不应改变增量指纹");
    let f5c = unit_fingerprint(&dir, &settings, None, true, "note-1", None);
    assert_ne!(f5b, f5c, "不排除时说明文件应计入目录统计");
    fs::remove_file(&note_path).unwrap();

    // 源目录变动（新增文件）→ 指纹变化
    fs::write(dir.join("b.txt"), b"x").unwrap();
    let f6 = unit_fingerprint(&dir, &settings, None, true, "note-1", Some(&note_path));
    assert_ne!(f1, f6);

    // 设置 / 密码变化 → 指纹变化
    let mut s2 = settings.clone();
    s2.comment = false;
    let f7 = unit_fingerprint(&dir, &s2, None, true, "note-1", Some(&note_path));
    assert_ne!(f1, f7);
    let f8 = unit_fingerprint(&dir, &settings, Some("pw"), true, "note-1", Some(&note_path));
    assert_ne!(f1, f8);
}

#[test]
fn incremental_zip_export() {
    let src_root = temp_dir("src_inc");
    let out_root = temp_dir("out_inc");
    build_source(&src_root);

    let tree = scan_tree(&src_root).expect("扫描失败");
    let detected = detect(&tree, &Overrides::default());
    let mut entries: Vec<Entry> = detected.units.iter().map(Entry::from_unit).collect();
    assert_eq!(entries.len(), 2);
    for e in &mut entries {
        e.selected = true;
    }

    let mut settings = ArchiveSettings {
        format: "zip".into(),
        volume_mib: 900,
        comment: true,
        encrypt_names: true,
        // 覆盖开启：让「增量跳过」不被「不覆盖跳过」掩盖
        overwrite: true,
        write_notes: true,
        incremental: true,
    };
    let cancel = AtomicBool::new(false);

    // 1) 首次导出：无清单 → 全量创建，指纹写入清单
    let p1 = run_zip_export(&src_root, &out_root, &settings, &entries, &cancel, &|_| {});
    assert_eq!(p1.created, 2, "{:?}", p1);
    assert_eq!(p1.failed, 0);
    let m1 = manifest::load(&out_root).expect("应写清单");
    assert_eq!(m1.files.len(), 2);
    assert!(
        m1.files.iter().all(|f| !f.fingerprint.is_empty()),
        "创建后应写入非空指纹: {:?}",
        m1.files
    );

    let a_zip = out_root.join("001 测试作品/画集/单位A/单位A.zip");
    let note_a = src_root.join("001 测试作品/画集/单位A/单位A.txt");
    let zip_mtime = manifest::mtime_ms(&a_zip);
    let note_mtime = manifest::mtime_ms(&note_a);

    // 2) 未变动 → 全部增量跳过（压缩包与说明文件均不重写）
    let msgs = std::cell::RefCell::new(Vec::new());
    let p2 = run_zip_export(&src_root, &out_root, &settings, &entries, &cancel, &|p| {
        if p.phase == "unit" && p.status == "skipped" {
            msgs.borrow_mut().push(p.message.clone());
        }
    });
    assert_eq!(p2.skipped, 2, "{:?}", p2);
    assert_eq!(p2.created, 0, "{:?}", p2);
    assert_eq!(
        *msgs.borrow(),
        vec!["增量跳过（未变动）".to_string(), "增量跳过（未变动）".to_string()],
        "跳过事件应带增量说明"
    );
    assert_eq!(manifest::mtime_ms(&a_zip), zip_mtime, "跳过不应重写压缩包");
    assert_eq!(manifest::mtime_ms(&note_a), note_mtime, "跳过不应重写说明文件");

    // 3) 单位A 源变动（新增文件）→ 仅 A 重建
    fs::write(
        src_root.join("001 测试作品/画集/单位A").join("extra.txt"),
        b"new",
    )
    .unwrap();
    let p3 = run_zip_export(&src_root, &out_root, &settings, &entries, &cancel, &|_| {});
    assert_eq!((p3.created, p3.skipped), (1, 1), "{:?}", p3);

    // 4) 条目备注变化 → 说明正文变化 → 仅 B 重建
    entries[1].notes = "导出前新改的备注".into();
    let p4 = run_zip_export(&src_root, &out_root, &settings, &entries, &cancel, &|_| {});
    assert_eq!((p4.created, p4.skipped), (1, 1), "{:?}", p4);

    // 5) 非增量导出重建后 → 指纹作废（写空），防止旧指纹被误用
    settings.incremental = false;
    let p5 = run_zip_export(&src_root, &out_root, &settings, &entries, &cancel, &|_| {});
    assert_eq!(p5.created, 2, "{:?}", p5);
    let m5 = manifest::load(&out_root).unwrap();
    assert!(
        m5.files.iter().all(|f| f.fingerprint.is_empty()),
        "非增量重建应作废指纹: {:?}",
        m5.files
    );

    // 6) 再开增量：空指纹不可跳过 → 全量重建并重新记指纹
    settings.incremental = true;
    let p6 = run_zip_export(&src_root, &out_root, &settings, &entries, &cancel, &|_| {});
    assert_eq!(p6.created, 2, "{:?}", p6);
    let m6 = manifest::load(&out_root).unwrap();
    assert!(m6.files.iter().all(|f| !f.fingerprint.is_empty()));

    // 7) 最终清单校验仍一致
    let report = manifest::verify(&out_root, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    assert_eq!(report.ok, 2, "{:?}", report);
}
