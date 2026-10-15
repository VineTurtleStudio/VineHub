use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
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
fn zip_encrypted_entry_is_not_corrupt() {
    // 用户反馈：加密压缩包无密码无法校验内容，加密本身不是损坏
    let dir = temp_dir("zip-enc");
    let path = make_zip(&dir, "enc.zip", b"secret payload for encryption test");

    // 把本地头与中央目录里的加密标志位（通用标志 bit0）置 1，
    // 模拟加密条目（无需真正加密内容，读取前就会被跳过）。
    let mut data = fs::read(&path).unwrap();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        if &data[i..i + 4] == b"PK\x03\x04" {
            data[i + 6] |= 0x01; // 本地头通用标志 bit0
        } else if &data[i..i + 4] == b"PK\x01\x02" {
            data[i + 8] |= 0x01; // 中央目录通用标志 bit0
        }
        i += 1;
    }
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_ne!(
        r.worst(),
        Severity::Corrupt,
        "加密条目不应判为损坏：{:?}",
        r.findings
    );
    assert!(
        codes(&r).contains(&"ZIP_ENCRYPTED"),
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
fn repeated_fill_file_is_warning() {
    let dir = temp_dir("repfill");
    let path = dir.join("fill.bin");
    // 00/FF 交替填充：非全零（否则走 ALL_ZERO），但仅两种字节值
    let data: Vec<u8> = (0..16 * 1024).map(|i| if i % 2 == 0 { 0 } else { 0xFF }).collect();
    fs::write(&path, data).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Warning, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"REPEATED_FILL"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn mostly_zero_with_header_is_repeated_fill() {
    let dir = temp_dir("repfill-hdr");
    let path = dir.join("image.img");
    // 头部少量杂字节 + 主体零填充：全零判定不适用，靠重复填充判定兜底
    let mut data: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
    data.extend(std::iter::repeat_n(0u8, 64 * 1024));
    fs::write(&path, data).unwrap();

    let r = verify_file(&path, &opts());
    assert!(
        codes(&r).contains(&"REPEATED_FILL"),
        "findings: {:?}",
        r.findings
    );
    assert_eq!(r.worst(), Severity::Warning, "findings: {:?}", r.findings);
}

#[test]
fn normal_binary_is_not_flagged_as_repeated_fill() {
    let dir = temp_dir("no-repfill");
    let path = dir.join("rand.bin");
    // 确定性 xorshift 伪随机内容：字节分布均匀，top-2 占比约 1%，不应误报
    let mut x: u32 = 0x1234_5678;
    let data: Vec<u8> = (0..16 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xFF) as u8
        })
        .collect();
    fs::write(&path, data).unwrap();

    let r = verify_file(&path, &opts());
    assert!(
        !codes(&r).contains(&"REPEATED_FILL"),
        "findings: {:?}",
        r.findings
    );
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

#[test]
fn repeated_fill_is_skipped_when_magic_present() {
    let dir = temp_dir("repfill-magic");
    let path = dir.join("fake.bmp");
    // 有魔数（BM）的文件不做重复填充判定：双色位图等合法内容也会呈双字节分布
    let mut data = b"BM".to_vec();
    data.extend(std::iter::repeat_n(0u8, 62)); // 假 BMP 文件头，凑齐魔数探测
    data.extend((0..16 * 1024).map(|i| if i % 2 == 0 { 0 } else { 0xFF }));
    fs::write(&path, data).unwrap();

    let r = verify_file(&path, &opts());
    assert!(
        !codes(&r).contains(&"REPEATED_FILL"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn flac_large_file_is_structurally_scanned_not_skipped() {
    // 稀疏文件：513 MiB 几乎不占磁盘。新行为：不再跳过，改用流式滑动窗口做
    // 逐帧结构遍历；零内容不是 fLaC → 流式扫描在偏移 0 报 FLAC_BAD_MAGIC（损坏）。
    let dir = temp_dir("flac-huge");
    let path = dir.join("huge.flac");
    let f = File::create(&path).unwrap();
    f.set_len(513 * 1024 * 1024).unwrap();
    drop(f);

    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert!(
        !cs.contains(&"FLAC_TOO_LARGE"),
        "大文件不应再被跳过：{:?}",
        r.findings
    );
    assert!(
        cs.contains(&"FLAC_BAD_MAGIC"),
        "流式结构扫描应识别出非 fLaC 头：{:?}",
        r.findings
    );
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
}

#[test]
fn flac_large_file_deep_still_runs_structural_scan_first() {
    // 深度解码不跳过结构层：稀疏零文件先被流式结构扫描判为 FLAC_BAD_MAGIC，
    // 结构层已损坏则不再做 MD5（不会出现 FLAC_NO_MD5）。
    let dir = temp_dir("flac-huge-deep");
    let path = dir.join("huge.flac");
    let f = File::create(&path).unwrap();
    f.set_len(513 * 1024 * 1024).unwrap();
    drop(f);

    let mut o = opts();
    o.deep = true;
    let r = verify_file(&path, &o);
    let cs = codes(&r);
    assert!(
        !cs.contains(&"FLAC_TOO_LARGE"),
        "强制深度解码时更不应跳过：{:?}",
        r.findings
    );
    assert!(
        cs.contains(&"FLAC_BAD_MAGIC"),
        "结构扫描应先于 MD5 执行：{:?}",
        r.findings
    );
    assert!(
        !cs.contains(&"FLAC_NO_MD5"),
        "结构层已损坏则不应再做 MD5：{:?}",
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
    let reports = walker::scan(&files, &opts(), &cancel, |_, _, _| {});
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

// ---------------------------------------------------------------------------
// 超过旧 512 MiB 上限的稀疏大文件仍然被完整验证（流式结构遍历，无大小跳过）
// ---------------------------------------------------------------------------

/// 稀疏大文件：头部写字节，其余空间展开为零（不占磁盘）
const BIG: u64 = 513 * 1024 * 1024;

fn sparse(path: &Path, head: &[u8], total: u64) {
    let mut f = File::create(path).unwrap();
    f.write_all(head).unwrap();
    f.set_len(total).unwrap();
}

#[test]
fn jpeg_oversized_file_is_still_verified() {
    let dir = temp_dir("jpeg-big");
    let path = dir.join("big.jpg");
    sparse(&path, &[0xFF, 0xD8, 0x05], BIG); // SOI 后直接是坏标记
    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(cs.contains(&"JPEG_BAD_MARKER"), "findings: {:?}", r.findings);
    assert!(!cs.contains(&"JPEG_TOO_LARGE"), "大小跳过码应已移除");
}

#[test]
fn gif_oversized_file_is_still_verified() {
    let dir = temp_dir("gif-big");
    let path = dir.join("big.gif");
    // 签名 + 7 字节逻辑屏幕描述符（packed=0 → 无全局颜色表），之后应是块标识
    sparse(&path, b"GIF89a\x01\x00\x01\x00\x00\x00\x00", BIG);
    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(cs.contains(&"GIF_BAD_BLOCK"), "findings: {:?}", r.findings);
    assert!(!cs.contains(&"IMAGE_TOO_LARGE"), "大小跳过码应已移除");
}

#[test]
fn bmp_oversized_file_is_still_verified() {
    let dir = temp_dir("bmp-big");
    let path = dir.join("big.bmp");
    let mut head = vec![0u8; 54];
    head[0..2].copy_from_slice(b"BM");
    head[2..6].copy_from_slice(&u32::MAX.to_le_bytes()); // 声明 4 GiB > 实际
    head[10..14].copy_from_slice(&54u32.to_le_bytes()); // 像素数据偏移
    head[14..18].copy_from_slice(&40u32.to_le_bytes()); // DIB 头
    sparse(&path, &head, BIG);
    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(cs.contains(&"BMP_TRUNCATED"), "findings: {:?}", r.findings);
    assert!(!cs.contains(&"IMAGE_TOO_LARGE"), "大小跳过码应已移除");
}

#[test]
fn mp3_oversized_file_is_still_verified() {
    let dir = temp_dir("mp3-big");
    let path = dir.join("big.mp3");
    let frame = [0xFFu8, 0xFB, 0x90, 0x00]; // MPEG1 L3 128kbps/44.1kHz，帧长 417
    {
        let mut f = File::create(&path).unwrap();
        f.write_all(&frame).unwrap();
        f.set_len(BIG).unwrap();
        // 1 MiB 处放第二帧：帧链在 417 处断裂后只需扫描 1 MiB 即可恢复
        f.seek(SeekFrom::Start(1 << 20)).unwrap();
        f.write_all(&frame).unwrap();
    }
    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(cs.contains(&"MP3_GARBAGE"), "findings: {:?}", r.findings);
    assert!(!cs.contains(&"MP3_TOO_LARGE"), "大小跳过码应已移除");
}

#[test]
fn gzip_oversized_compressed_gets_skip_note() {
    let dir = temp_dir("gzip-big");
    let path = dir.join("big.gz");
    sparse(&path, &[0x1F, 0x8B, 0x08, 0x00, 0, 0, 0, 0, 0, 0], BIG);
    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert!(
        cs.contains(&"GZIP_CRC_SKIPPED"),
        "应记 Ok 级跳过说明，findings: {:?}",
        r.findings
    );
    assert!(!cs.contains(&"GZIP_TOO_LARGE"), "大小跳过码应已移除");
    assert!(
        r.worst().rank() <= Severity::Warning.rank(),
        "只允许熵类提示，findings: {:?}",
        r.findings
    );
}

#[test]
fn sqlite_oversized_gets_check_skip_note() {
    let dir = temp_dir("sqlite-big");
    let path = dir.join("big.sqlite");
    sparse(&path, b"SQLite format 3\0", BIG);
    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert!(
        cs.contains(&"SQLITE_CHECK_SKIPPED"),
        "应记 Ok 级跳过说明，findings: {:?}",
        r.findings
    );
    assert!(
        r.worst().rank() <= Severity::Warning.rank(),
        "只允许熵类提示，findings: {:?}",
        r.findings
    );
}

#[test]
fn sevenz_oversized_header_is_streaming_crc_checked() {
    let dir = temp_dir("7z-big");
    let path = dir.join("big.7z");
    let hdr_size: u64 = 65 * 1024 * 1024; // 超过旧的 64 MiB 跳过上限
    // 后续头内容为全零（稀疏），其 CRC32 需与签名头声明一致
    let next_crc = {
        let mut h = crc32fast::Hasher::new();
        let z = vec![0u8; 1 << 20];
        let mut left = hdr_size;
        while left > 0 {
            let n = left.min(1 << 20) as usize;
            h.update(&z[..n]);
            left -= n as u64;
        }
        h.finalize()
    };
    let mut head = vec![0u8; 32];
    head[..6].copy_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);
    head[6] = 0;
    head[7] = 4;
    head[12..20].copy_from_slice(&0u64.to_le_bytes()); // next_off = 0
    head[20..28].copy_from_slice(&hdr_size.to_le_bytes());
    head[28..32].copy_from_slice(&next_crc.to_le_bytes());
    let start_crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&head[12..32]);
        h.finalize()
    };
    head[8..12].copy_from_slice(&start_crc.to_le_bytes());
    sparse(&path, &head, 32 + hdr_size);

    let r = verify_file(&path, &opts());
    let cs = codes(&r);
    assert!(
        !cs.contains(&"SEVENZ_HEADER_TOO_LARGE"),
        "超大后续头应流式校验 CRC，findings: {:?}",
        r.findings
    );
    assert!(
        r.worst().rank() <= Severity::Warning.rank(),
        "CRC 匹配应为正常/熵类提示，findings: {:?}",
        r.findings
    );
}

#[test]
fn gzip_large_decoded_output_is_fully_verified() {
    // 513 MiB 全零：旧实现解压超 512 MiB 即中止（GZIP_TOO_LARGE），现在应完整解压校验
    let dir = temp_dir("gzip-decode");
    let src = dir.join("zeros.bin");
    sparse(&src, &[], BIG);
    let gz = dir.join("zeros.gz");
    {
        let mut r = File::open(&src).unwrap();
        let mut e = flate2::write::GzEncoder::new(
            File::create(&gz).unwrap(),
            flate2::Compression::fast(),
        );
        std::io::copy(&mut r, &mut e).unwrap();
        e.finish().unwrap();
    }
    let r = verify_file(&gz, &opts());
    let cs = codes(&r);
    assert!(
        !cs.contains(&"GZIP_TOO_LARGE") && !cs.contains(&"GZIP_DECODE_LIMIT"),
        "513 MiB 解压应完整执行，findings: {:?}",
        r.findings
    );
    assert!(
        r.worst().rank() <= Severity::Warning.rank(),
        "findings: {:?}",
        r.findings
    );
}
