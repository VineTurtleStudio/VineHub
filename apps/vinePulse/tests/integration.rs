use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use image::{Rgb, RgbImage};
use vine_pulse::{Mode, Opts, Report, Severity, hash, verify_file, walker};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-test-{}-{}",
        tag,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_png(dir: &Path, name: &str) -> PathBuf {
    let img = RgbImage::from_fn(16, 16, |x, y| Rgb([(x * 8) as u8, (y * 8) as u8, 128]));
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

fn make_jpeg(dir: &Path, name: &str) -> PathBuf {
    let img = RgbImage::from_fn(32, 32, |x, y| Rgb([(x * 4) as u8, (y * 4) as u8, 64]));
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

fn make_zip(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    let path = dir.join(name);
    let file = File::create(&path).unwrap();
    let mut w = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("data.txt", opts).unwrap();
    w.write_all(content).unwrap();
    w.finish().unwrap();
    path
}

fn codes(r: &Report) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

fn opts() -> Opts {
    Opts::default()
}

#[test]
fn png_valid_is_ok() {
    let dir = temp_dir("png-ok");
    let path = make_png(&dir, "ok.png");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "PNG");
}

#[test]
fn png_bad_crc_detected() {
    let dir = temp_dir("png-crc");
    let path = make_png(&dir, "bad-crc.png");
    let mut data = fs::read(&path).unwrap();
    // 定位第一个 IDAT 块并翻转其数据首字节
    let pos = data
        .windows(4)
        .position(|w| w == b"IDAT")
        .expect("应存在 IDAT 块");
    data[pos + 4] ^= 0xFF;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"PNG_BAD_CRC"),
        "findings: {:?}",
        r.findings
    );
    let f = r.findings.iter().find(|f| f.code == "PNG_BAD_CRC").unwrap();
    assert!(f.offset.is_some(), "CRC 问题应带字节偏移");
}

#[test]
fn png_truncated_detected() {
    let dir = temp_dir("png-trunc");
    let path = make_png(&dir, "trunc.png");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 10]).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"PNG_NO_IEND"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn png_broken_signature_detected() {
    let dir = temp_dir("png-sig");
    let path = make_png(&dir, "bad-sig.png");
    let mut data = fs::read(&path).unwrap();
    for b in data.iter_mut().take(8) {
        *b = 0x55;
    }
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"PNG_BAD_SIG"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn jpeg_valid_is_ok() {
    let dir = temp_dir("jpg-ok");
    let path = make_jpeg(&dir, "ok.jpg");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "JPEG");
}

#[test]
fn jpeg_missing_eoi_detected() {
    let dir = temp_dir("jpg-eoi");
    let path = make_jpeg(&dir, "no-eoi.jpg");
    let data = fs::read(&path).unwrap();
    assert_eq!(
        &data[data.len() - 2..],
        &[0xFF, 0xD9],
        "JPEG 应以 FFD9 结束"
    );
    fs::write(&path, &data[..data.len() - 2]).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"JPEG_NO_EOI"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn zip_valid_is_ok() {
    let dir = temp_dir("zip-ok");
    let path = make_zip(&dir, "ok.zip", b"vine pulse valid archive content");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "ZIP");
}

#[test]
fn zip_bad_crc_detected() {
    let dir = temp_dir("zip-crc");
    let content = b"vine-pulse-corrupt-sample-0123456789";
    let path = make_zip(&dir, "bad-crc.zip", content);
    let mut data = fs::read(&path).unwrap();

    let pos = data
        .windows(content.len())
        .position(|w| w == content)
        .expect("stored 条目内容应能在压缩包中明文找到");
    data[pos] ^= 0xFF;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs = codes(&r);
    assert!(
        cs.contains(&"ZIP_BAD_CRC") || cs.contains(&"ZIP_DECOMPRESS_FAILED"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn zip_truncated_detected() {
    let dir = temp_dir("zip-trunc");
    let path = make_zip(&dir, "trunc.zip", b"some payload for truncation test");
    let data = fs::read(&path).unwrap();
    // 砍掉整个 EOCD（22 字节）及以上
    fs::write(&path, &data[..data.len() - 40]).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"ZIP_NO_EOCD"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn empty_file_is_warning() {
    let dir = temp_dir("empty");
    let path = dir.join("empty.txt");
    File::create(&path).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Warning, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"EMPTY_FILE"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn all_zero_file_is_warning() {
    let dir = temp_dir("zero");
    let path = dir.join("pad.bin");
    fs::write(&path, vec![0u8; 16 * 1024]).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Warning, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"ALL_ZERO"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn magic_extension_mismatch_detected() {
    let dir = temp_dir("mismatch");
    let zip_path = make_zip(&dir, "real.zip", b"content disguised as png file");
    let disguised = dir.join("fake.png");
    fs::copy(&zip_path, &disguised).unwrap();

    let r = verify_file(&disguised, &opts());
    assert_eq!(r.worst(), Severity::Warning, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"MAGIC_MISMATCH"),
        "findings: {:?}",
        r.findings
    );
    assert_eq!(r.file_type, "ZIP", "应按魔数识别出真实类型");
}

#[test]
fn sha256_manifest_ok_and_mismatch() {
    let dir = temp_dir("manifest");
    let path = dir.join("data.bin");
    fs::write(&path, b"hello vine pulse").unwrap();

    let digest = hash::compute_hash(&path, hash::HashKind::Sha256).unwrap();
    let manifest = dir.join("data.sha256");
    fs::write(&manifest, format!("{digest} *data.bin\n")).unwrap();

    let entries = hash::parse_manifest(&manifest).unwrap();
    assert_eq!(entries.len(), 1);
    let o = Opts {
        sidecar_hash: false,
        manifest_index: Some(Arc::new(hash::index_manifest(&entries))),
        ..Default::default()
    };

    let r = verify_file(&path, &o);
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.mode, Mode::Hash);
    assert!(codes(&r).contains(&"HASH_OK"), "findings: {:?}", r.findings);

    // 篡改文件后应判定为损坏
    fs::write(&path, b"hello vine pulse!").unwrap();
    let r2 = verify_file(&path, &o);
    assert_eq!(r2.worst(), Severity::Corrupt, "findings: {:?}", r2.findings);
    assert!(
        codes(&r2).contains(&"HASH_MISMATCH"),
        "findings: {:?}",
        r2.findings
    );
}

#[test]
fn sfv_manifest_parsed() {
    let dir = temp_dir("sfv");
    let path = dir.join("file.txt");
    fs::write(&path, b"sfv content").unwrap();

    let crc = hash::compute_hash(&path, hash::HashKind::Crc32).unwrap();
    let manifest = dir.join("list.sfv");
    fs::write(
        &manifest,
        format!("; generated by test\r\nfile.txt {crc}\r\n"),
    )
    .unwrap();

    let entries = hash::parse_manifest(&manifest).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, hash::HashKind::Crc32);
    assert_eq!(entries[0].expected, crc.to_ascii_lowercase());
}

#[test]
fn sidecar_hash_detected() {
    let dir = temp_dir("sidecar");
    let path = dir.join("photo.png");
    make_png(&dir, "photo.png");
    let digest = hash::compute_hash(&path, hash::HashKind::Sha256).unwrap();
    fs::write(
        dir.join("photo.png.sha256"),
        format!("{digest}  photo.png\n"),
    )
    .unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.mode, Mode::Hash, "findings: {:?}", r.findings);
    assert!(codes(&r).contains(&"HASH_OK"), "findings: {:?}", r.findings);

    // 篡改后再扫
    let mut data = fs::read(&path).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0x01;
    fs::write(&path, &data).unwrap();
    let r2 = verify_file(&path, &opts());
    assert_eq!(r2.mode, Mode::Hash);
    assert!(
        codes(&r2).contains(&"HASH_MISMATCH"),
        "findings: {:?}",
        r2.findings
    );
}

#[test]
fn walker_collects_and_scans() {
    let dir = temp_dir("walker");
    fs::create_dir_all(dir.join("sub")).unwrap();
    make_png(&dir, "a.png");
    make_jpeg(&dir.join("sub"), "b.jpg");
    make_zip(&dir, "c.zip", b"nested archive");
    fs::write(dir.join("plain.txt"), "plain").unwrap();

    let (files, errors) = walker::collect_files(std::slice::from_ref(&dir));
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(files.len(), 4, "files: {files:?}");

    let cancel = AtomicBool::new(false);
    let reports = walker::scan(&files, &opts(), &cancel, |_, _| {});
    assert_eq!(reports.len(), 4);
    // 扫描结果按路径排序
    let mut sorted = reports.clone();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        reports.iter().map(|r| &r.path).collect::<Vec<_>>(),
        sorted.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
    assert!(
        reports.iter().all(|r| r.is_ok()),
        "生成的样本应全部正常: {:?}",
        reports
            .iter()
            .map(|r| (&r.path, &r.findings))
            .collect::<Vec<_>>()
    );
}

#[test]
fn report_serializes_to_json() {
    let dir = temp_dir("json");
    let path = make_png(&dir, "ok.png");
    let r = verify_file(&path, &opts());
    let s = serde_json::to_string_pretty(&[r]).unwrap();
    assert!(s.contains("\"file_type\": \"PNG\""), "json: {s}");
    assert!(s.contains("\"findings\""), "json: {s}");
    assert!(s.contains("\"mode\""), "json: {s}");
}

#[test]
fn text_file_named_zip_is_corrupt() {
    let dir = temp_dir("fake-zip");
    let path = dir.join("broken.zip");
    let mut f = File::create(&path).unwrap();
    f.write_all(b"not really a zip at all, just text pretending")
        .unwrap();
    drop(f);

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs = codes(&r);
    assert!(cs.contains(&"MAGIC_MISMATCH"), "findings: {:?}", r.findings);
    assert!(cs.contains(&"ZIP_NO_EOCD"), "findings: {:?}", r.findings);
}
