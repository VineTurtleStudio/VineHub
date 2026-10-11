use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use vine_press_lib::archive::{create_rar, with_backup, ArchiveRequest};

fn find_rar() -> Option<String> {
    for c in [
        "C:\\Program Files\\WinRAR\\Rar.exe",
        "C:\\Program Files (x86)\\WinRAR\\Rar.exe",
    ] {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    None
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("vinepress_test").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("sub")).unwrap();
    d
}

#[test]
#[ignore = "依赖本机 WinRAR"]
fn packs_single_unit_with_password_comment_and_subdir() {
    let rar = match find_rar() {
        Some(r) => r,
        None => {
            eprintln!("skip: 未找到 Rar.exe");
            return;
        }
    };

    let src = temp_dir("unit");
    fs::write(src.join("说明.txt"), "内容文件").unwrap();
    fs::write(src.join("sub").join("a.jpg"), [0xFFu8, 0xD8, 0xFF, 0xD9]).unwrap();

    let out_dir = temp_dir("out");
    let out = out_dir.join("unit [密码测试].rar");

    let req = ArchiveRequest {
        rar_path: rar,
        src_dir: src.clone(),
        out_archive: out.clone(),
        password: Some("密码测试".into()),
        encrypt_names: true,
        volume_mib: 0,
        comment: Some("注释内容测试\n第二行".into()),
        overwrite: true,
    };
    let cancel = AtomicBool::new(false);
    let mut last = 0u8;
    let outcome = create_rar(&req, |p| last = p, &cancel).expect("创建压缩包失败");

    assert_eq!(outcome.status, "created");
    assert!(out.exists(), "压缩包应存在");
    assert!(last == 100, "进度应到达 100，实际 {}", last);

    // 未加密列表应被拒绝（头已加密）
    let list = std::process::Command::new(&req.rar_path)
        .arg("l")
        .arg(&out)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&list.stdout);
    let stderr = String::from_utf8_lossy(&list.stderr);
    assert!(
        !text.contains("a.jpg") && !stderr.contains("a.jpg"),
        "头应已加密"
    );

    // 加密后应能看到子目录结构（WinRAR 控制台输出为本地 ANSI/GBK）
    let list2 = std::process::Command::new(&req.rar_path)
        .arg("l")
        .arg(format!("-p{}", "密码测试"))
        .arg(&out)
        .output()
        .unwrap();
    let text2 = encoding_rs::GB18030
        .decode(&list2.stdout)
        .0
        .into_owned();
    assert!(text2.contains("a.jpg"), "应包含子目录文件: {}", text2);
    assert!(text2.contains("说明.txt"), "应包含说明文件: {}", text2);
    assert!(text2.contains("sub"), "应保留子目录: {}", text2);
    // 注释（GBK 往返）应出现在列表输出中
    assert!(
        text2.contains("注释内容测试") && text2.contains("第二行"),
        "注释应正确写入: {}",
        &text2[..text2.len().min(400)]
    );

    // 不覆盖时应跳过已存在
    let req_no_overwrite = ArchiveRequest {
        rar_path: req.rar_path.clone(),
        src_dir: src.clone(),
        out_archive: out.clone(),
        password: Some("密码测试".into()),
        encrypt_names: true,
        volume_mib: 0,
        comment: Some("注释内容测试\n第二行".into()),
        overwrite: false,
    };
    let outcome2 = create_rar(&req_no_overwrite, |_| {}, &cancel).unwrap();
    assert_eq!(outcome2.status, "skipped");

    // 取消
    let cancel2 = AtomicBool::new(true);
    let outcome3 = create_rar(&req, |_| {}, &cancel2).unwrap();
    // overwrite=true 会先把旧包改名备份，取消后必须原样恢复
    assert!(outcome3.status == "cancelled" || outcome3.status == "created");
    assert!(out.exists(), "取消后旧压缩包必须恢复");
    assert!(
        !out_dir.join("unit [密码测试].rar.prevbak").exists(),
        "不应残留备份文件"
    );
}

#[test]
fn with_backup_commits_new_archive_on_success() {
    let dir = temp_dir("backup_ok");
    let out = dir.join("pkg.rar");
    fs::write(&out, b"old").unwrap();

    let value = with_backup(&out, || {
        fs::write(&out, b"new").unwrap();
        Ok((7u32, true))
    })
    .expect("成功应返回值");

    assert_eq!(value, 7);
    assert_eq!(fs::read(&out).unwrap(), b"new", "应保留新产物");
    assert!(!dir.join("pkg.rar.prevbak").exists(), "成功后应删除备份");
}

#[test]
fn with_backup_restores_old_archive_on_failure() {
    let dir = temp_dir("backup_fail");
    let out = dir.join("pkg.rar");
    fs::write(&out, b"old").unwrap();

    let result: Result<u32, String> = with_backup(&out, || {
        fs::write(&out, b"partial").unwrap();
        Err("创建失败".into())
    });

    assert!(result.is_err());
    assert_eq!(fs::read(&out).unwrap(), b"old", "失败必须恢复旧包");
    assert!(!dir.join("pkg.rar.prevbak").exists(), "恢复后不应残留备份");
}

#[test]
fn with_backup_restores_on_cancel_and_handles_volumes() {
    let dir = temp_dir("backup_cancel");
    let out = dir.join("pkg.rar");
    fs::write(&out, b"old-single").unwrap();
    fs::write(dir.join("pkg.part1.rar"), b"old-vol1").unwrap();
    fs::write(dir.join("pkg.part2.rar"), b"old-vol2").unwrap();

    let value = with_backup(&out, || {
        fs::write(&out, b"partial").unwrap();
        Ok((("cancelled".to_string()), false))
    })
    .expect("取消不是错误");

    assert_eq!(value, "cancelled");
    assert_eq!(fs::read(&out).unwrap(), b"old-single");
    assert_eq!(fs::read(dir.join("pkg.part1.rar")).unwrap(), b"old-vol1");
    assert_eq!(fs::read(dir.join("pkg.part2.rar")).unwrap(), b"old-vol2");
    assert!(!dir.join("pkg.rar.prevbak").exists());
    assert!(!dir.join("pkg.part1.rar.prevbak").exists());
    assert!(!dir.join("pkg.part2.rar.prevbak").exists());
}

#[test]
fn with_backup_works_without_existing_archives() {
    let dir = temp_dir("backup_empty");
    let out = dir.join("fresh.zip");
    let value = with_backup(&out, || {
        fs::write(&out, b"brand new").unwrap();
        Ok((1u8, true))
    })
    .unwrap();
    assert_eq!(value, 1);
    assert_eq!(fs::read(&out).unwrap(), b"brand new");
}
