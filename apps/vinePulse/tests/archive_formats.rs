use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-archive-{}-{}",
        tag,
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// 从内置真实样本拷一份，供破坏性测试使用
fn copy_fixture(name: &str, tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    let dst = dir.join(name);
    fs::copy(fixture(name), &dst).unwrap();
    dst
}

fn codes(r: &Report) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

fn opts() -> Opts {
    Opts::default()
}

fn assert_corrupt(r: &Report, code: &str) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

fn assert_warning(r: &Report, code: &str) {
    assert_ne!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

fn crc32(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

// ---------------------------------------------------------------------------
// gzip
// ---------------------------------------------------------------------------

#[test]
fn gzip_valid_is_ok() {
    let path = copy_fixture("gzip_ok.gz", "gz-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "gzip");
}

#[test]
fn gzip_truncated_is_corrupt() {
    let path = copy_fixture("gzip_ok.gz", "gz-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 5]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "GZIP_TRUNCATED");
}

#[test]
fn gzip_tampered_trailer_crc_is_corrupt() {
    let path = copy_fixture("gzip_ok.gz", "gz-crc");
    let mut data = fs::read(&path).unwrap();
    let n = data.len();
    data[n - 8] ^= 0xFF; // 尾部 CRC32 字段
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "GZIP_BAD_CRC");
}

#[test]
fn gzip_bad_compression_method_is_corrupt() {
    let path = copy_fixture("gzip_ok.gz", "gz-cm");
    let mut data = fs::read(&path).unwrap();
    data[2] = 9;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "GZIP_BAD_CM");
}

/// 在固定头之后插入合法 FHCRC（CRC32 低 16 位）
fn gzip_with_fhcrc(raw: &[u8], good: bool) -> Vec<u8> {
    let mut fixed = raw[..10].to_vec();
    fixed[3] |= 0x02;
    let mut crc16 = (crc32(&fixed) & 0xFFFF) as u16;
    if !good {
        crc16 ^= 0xFFFF;
    }
    let mut out = fixed;
    out.extend_from_slice(&crc16.to_le_bytes());
    out.extend_from_slice(&raw[10..]);
    out
}

#[test]
fn gzip_valid_fhcrc_is_ok() {
    let dir = temp_dir("gz-fhcrc-ok");
    let raw = fs::read(fixture("gzip_ok.gz")).unwrap();
    let path = dir.join("fhcrc_ok.gz");
    fs::write(&path, gzip_with_fhcrc(&raw, true)).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

#[test]
fn gzip_bad_fhcrc_is_corrupt() {
    let dir = temp_dir("gz-fhcrc-bad");
    let raw = fs::read(fixture("gzip_ok.gz")).unwrap();
    let path = dir.join("fhcrc_bad.gz");
    fs::write(&path, gzip_with_fhcrc(&raw, false)).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "GZIP_BAD_FHCRC");
}

#[test]
fn gzip_multi_member_is_ok() {
    let dir = temp_dir("gz-multi");
    let mut out = fs::read(fixture("gzip_ok.gz")).unwrap();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(b"second member").unwrap();
    out.extend_from_slice(&enc.finish().unwrap());

    let path = dir.join("multi.gz");
    fs::write(&path, &out).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

// ---------------------------------------------------------------------------
// tar
// ---------------------------------------------------------------------------

/// 重算并写回 512 字节块的校验和字段
fn fix_tar_checksum(block: &mut [u8]) {
    for b in &mut block[148..156] {
        *b = b' ';
    }
    let sum: u64 = block.iter().map(|&b| u64::from(b)).sum();
    let text = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(text.as_bytes());
}

#[test]
fn tar_valid_is_ok() {
    let path = copy_fixture("tar_ok.tar", "tar-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "tar");
}

#[test]
fn tar_checksum_tampered_is_corrupt() {
    let path = copy_fixture("tar_ok.tar", "tar-cksum");
    let mut data = fs::read(&path).unwrap();
    data[10] ^= 0xFF; // 名字字段，破坏块校验和
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TAR_BAD_CHECKSUM");
}

#[test]
fn tar_bad_size_field_is_corrupt() {
    let path = copy_fixture("tar_ok.tar", "tar-size");
    let mut data = fs::read(&path).unwrap();
    data[124..136].copy_from_slice(b"zzzzzzzzzzzz");
    fix_tar_checksum(&mut data[..512]);
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TAR_BAD_SIZE");
}

#[test]
fn tar_truncated_is_corrupt() {
    let path = copy_fixture("tar_ok.tar", "tar-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 2048]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TAR_TRUNCATED");
}

#[test]
fn tar_missing_end_marker_is_warning() {
    let path = copy_fixture("tar_ok.tar", "tar-noend");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 1024]).unwrap(); // 只留条目数据，去掉两个零块

    let r = verify_file(&path, &opts());
    assert_warning(&r, "TAR_NO_END");
}

#[test]
fn tar_only_zero_blocks_is_empty_warning() {
    let dir = temp_dir("tar-empty");
    let path = dir.join("empty.tar");
    fs::write(&path, vec![0u8; 1024]).unwrap();

    let r = verify_file(&path, &opts());
    assert_warning(&r, "TAR_EMPTY");
}

// ---------------------------------------------------------------------------
// rar
// ---------------------------------------------------------------------------

#[test]
fn rar4_valid_is_ok() {
    let path = copy_fixture("rar4_ok.rar", "rar4-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "RAR");
}

#[test]
fn rar5_valid_is_ok() {
    let path = copy_fixture("rar5_ok.rar", "rar5-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "RAR");
}

#[test]
fn rar4_truncated_is_corrupt() {
    let path = copy_fixture("rar4_ok.rar", "rar4-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 10]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RAR_TRUNCATED");
}

#[test]
fn rar5_truncated_is_corrupt() {
    let path = copy_fixture("rar5_ok.rar", "rar5-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 10]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RAR_TRUNCATED");
}

#[test]
fn rar4_tampered_header_is_corrupt() {
    let path = copy_fixture("rar4_ok.rar", "rar4-crc");
    let mut data = fs::read(&path).unwrap();
    data[60] ^= 0xFF; // 第二个块（文件头）的名字字段
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RAR_BAD_CRC");
}

#[test]
fn rar5_tampered_header_is_corrupt() {
    let path = copy_fixture("rar5_ok.rar", "rar5-crc");
    let mut data = fs::read(&path).unwrap();
    data[60] ^= 0xFF; // 第二个块（文件头）的块体
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RAR_BAD_CRC");
}

// ---------------------------------------------------------------------------
// 7z
// ---------------------------------------------------------------------------

/// 生成带正确 StartHeaderCRC / NextHeaderCRC 的 7z 骨架
fn build_7z(header: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"7z\xbc\xaf\x27\x1c");
    out.extend_from_slice(&[0x00, 0x04]); // version
    let mut start = Vec::new();
    start.extend_from_slice(&0u64.to_le_bytes()); // NextHeaderOffset
    start.extend_from_slice(&(header.len() as u64).to_le_bytes());
    start.extend_from_slice(&crc32(header).to_le_bytes());
    out.extend_from_slice(&crc32(&start).to_le_bytes());
    out.extend_from_slice(&start);
    out.extend_from_slice(header);
    out
}

#[test]
fn sevenz_synthetic_ok() {
    let dir = temp_dir("7z-ok");
    let path = dir.join("synthetic.7z");
    fs::write(&path, build_7z(b"fake seven-z header payload")).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "7z");
}

#[test]
fn sevenz_bad_start_crc_is_corrupt() {
    let dir = temp_dir("7z-startcrc");
    let mut data = build_7z(b"fake seven-z header payload");
    data[12] ^= 0xFF; // 破坏 StartHeader 覆盖区
    let path = dir.join("start_crc.7z");
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "SEVENZ_BAD_START_CRC");
}

#[test]
fn sevenz_bad_header_crc_is_corrupt() {
    let dir = temp_dir("7z-hdrcrc");
    let mut data = build_7z(b"fake seven-z header payload");
    let n = data.len();
    data[n - 1] ^= 0xFF; // 只改后续头内容，StartHeader 不受影响
    let path = dir.join("header_crc.7z");
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "SEVENZ_BAD_HEADER_CRC");
}

#[test]
fn sevenz_truncated_is_corrupt() {
    let dir = temp_dir("7z-trunc");
    let data = build_7z(b"fake seven-z header payload");
    let path = dir.join("trunc.7z");
    fs::write(&path, &data[..30]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "SEVENZ_TRUNCATED");
}
