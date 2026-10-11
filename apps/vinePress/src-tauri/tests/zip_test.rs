use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use vine_press_lib::archive::{create_zip, ArchiveRequest};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("vinepress_zip").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn make_src(dir: &PathBuf) {
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), b"hello vinePress").unwrap();
    fs::write(dir.join("sub").join("b.txt"), b"second").unwrap();
    fs::write(dir.join("说明.txt"), "中文内容".as_bytes()).unwrap();
    fs::write(dir.join("Thumbs.db"), b"junk").unwrap();
}

fn request(src: &PathBuf, out: &PathBuf, overwrite: bool) -> ArchiveRequest {
    ArchiveRequest {
        rar_path: String::new(),
        src_dir: src.clone(),
        out_archive: out.clone(),
        password: None,
        encrypt_names: false,
        volume_mib: 0,
        comment: Some("标题\n第二行".into()),
        overwrite,
    }
}

#[test]
fn zip_roundtrip_with_gbk_comment() {
    let root = temp_dir("basic");
    let src = root.join("单位A");
    make_src(&src);
    let out = root.join("out").join("单位A.zip");
    fs::create_dir_all(out.parent().unwrap()).unwrap();

    let req = request(&src, &out, false);
    let cancel = AtomicBool::new(false);
    let mut last = 0u8;
    let outcome = create_zip(&req, |p| last = p, &cancel).expect("打包失败");
    assert_eq!(outcome.status, "created");
    assert_eq!(outcome.archives.len(), 1);
    assert_eq!(last, 100, "进度应收在 100");
    assert!(out.is_file(), "应生成 {}", out.display());

    let file = File::open(&out).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
    assert!(names.contains(&"a.txt".to_string()), "{:?}", names);
    assert!(names.contains(&"sub/b.txt".to_string()), "zip 内用 / 分隔: {:?}", names);
    assert!(names.contains(&"说明.txt".to_string()), "{:?}", names);
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("Thumbs.db")),
        "Thumbs.db 应被跳过: {:?}",
        names
    );

    // 注释必须是 GBK 原始字节
    let (gbk, _, _) = encoding_rs::GB18030.encode("标题\n第二行");
    assert_eq!(archive.comment(), &gbk[..], "注释应按 GBK 写入");

    // 文件内容可读回
    let mut text = String::new();
    archive
        .by_name("a.txt")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "hello vinePress");
    drop(archive);

    // 已存在且不覆盖 → skipped
    let skipped = create_zip(&req, |_| {}, &cancel).unwrap();
    assert_eq!(skipped.status, "skipped");

    // 覆盖 → 重建
    let overwrite_req = request(&src, &out, true);
    let recreated = create_zip(&overwrite_req, |_| {}, &cancel).unwrap();
    assert_eq!(recreated.status, "created");

    // 取消 → 既有压缩包原样保留（不先删后建）
    let before = fs::read(&out).unwrap();
    let cancel_now = AtomicBool::new(true);
    let cancelled = create_zip(&overwrite_req, |_| {}, &cancel_now).unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(fs::read(&out).unwrap(), before, "取消后旧包必须完好");
    assert!(!out.with_extension("zip.prevbak").exists(), "备份不应残留");
}

#[test]
fn zip_rejects_missing_source_dir() {
    let root = temp_dir("missing");
    let req = request(&root.join("不存在"), &root.join("x.zip"), false);
    let cancel = AtomicBool::new(false);
    let err = create_zip(&req, |_| {}, &cancel).unwrap_err();
    assert!(err.contains("源目录不存在"), "{}", err);
}
