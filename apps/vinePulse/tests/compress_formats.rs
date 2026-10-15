use std::fs;
use std::path::PathBuf;

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-compress-{}-{}",
        tag,
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_case(tag: &str, name: &str, data: &[u8]) -> PathBuf {
    let path = temp_dir(tag).join(name);
    fs::write(&path, data).unwrap();
    path
}

fn opts() -> Opts {
    Opts::default()
}

fn codes(r: &Report) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

fn assert_ok(r: &Report, expected_type: &str) {
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, expected_type);
}

fn assert_corrupt(r: &Report, code: &str) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

/// 标准 CRC-32（IEEE），与 crc32fast::hash 等价
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ---------------------------------------------------------------------------
// xz
// ---------------------------------------------------------------------------

fn xz_file(bad_header_crc: bool, bad_footer_crc: bool, bad_tail_flags: bool) -> Vec<u8> {
    let flags = [0x00u8, 0x00];
    let mut b = vec![0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
    b.extend_from_slice(&flags);
    let h = crc32(&flags);
    if bad_header_crc {
        b.extend_from_slice(&(h ^ 1).to_le_bytes());
    } else {
        b.extend_from_slice(&h.to_le_bytes());
    }
    // 模拟压缩数据 + index（凑足长度让头尾不重叠）
    b.extend_from_slice(&[0u8; 8]);
    // 模拟尾部 12 字节 footer：CRC32 + backward_size + flags
    let backward = 8u32; // 假 index 大小
    let mut tail = Vec::new();
    tail.extend_from_slice(&backward.to_le_bytes());
    let tail_flags = if bad_tail_flags { [0x01, 0x00] } else { flags };
    tail.extend_from_slice(&tail_flags);
    let f = crc32(&tail);
    if bad_footer_crc {
        b.extend_from_slice(&(f ^ 1).to_le_bytes());
    } else {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b.extend_from_slice(&tail);
    b
}

#[test]
fn xz_valid_is_ok() {
    let path = write_case("xz-ok", "ok.xz", &xz_file(false, false, false));
    let r = verify_file(&path, &opts());
    assert_ok(&r, "xz");
}

#[test]
fn xz_bad_header_crc_is_corrupt() {
    let path = write_case("xz-hcrc", "hcrc.xz", &xz_file(true, false, false));
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "XZ_BAD_HEADER_CRC");
}

#[test]
fn xz_bad_footer_crc_is_corrupt() {
    let path = write_case("xz-fcrc", "fcrc.xz", &xz_file(false, true, false));
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "XZ_BAD_FOOTER_CRC");
}

#[test]
fn xz_truncated_is_corrupt() {
    // 魔数命中但不足 12 字节流头
    let mut v = vec![0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
    v.extend_from_slice(&[0u8; 5]);
    assert_eq!(v.len(), 11);
    let path = write_case("xz-trunc", "trunc.xz", &v);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "XZ_TRUNCATED");
}

// ---------------------------------------------------------------------------
// bzip2
// ---------------------------------------------------------------------------

fn bzip2_file(level: u8, good_eof: bool) -> Vec<u8> {
    let mut b = b"BZh".to_vec();
    b.push(level);
    // 首块 π 标志
    b.extend_from_slice(&[0x31, 0x41, 0x59, 0x26, 0x53, 0x59]);
    // 假数据
    b.extend_from_slice(&[0x00; 8]);
    // EOF 组合子 + combined CRC
    if good_eof {
        b.extend_from_slice(&[0x17, 0x72, 0x45, 0x38, 0x50, 0x90, 0x54, 0x17]);
    } else {
        b.extend_from_slice(&[0x00; 8]);
    }
    b.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    b
}

#[test]
fn bzip2_valid_is_ok() {
    let path = write_case("bz2-ok", "ok.bz2", &bzip2_file(b'9', true));
    let r = verify_file(&path, &opts());
    assert_ok(&r, "bzip2");
}

#[test]
fn bzip2_missing_eof_is_corrupt() {
    let path = write_case("bz2-eof", "noeof.bz2", &bzip2_file(b'9', false));
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "BZIP2_NO_EOF");
}

#[test]
fn bzip2_bad_block_magic_is_corrupt() {
    let mut b = bzip2_file(b'9', true);
    b[4] = 0x99; // 破坏首块 π 标志
    let path = write_case("bz2-blk", "badblk.bz2", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "BZIP2_BAD_BLOCK");
}

#[test]
fn bzip2_bad_level_by_extension_is_corrupt() {
    // 魔数要求 1-9：给 'A' → 魔数不命中，按扩展名进入验证器
    let mut b = bzip2_file(b'A', true);
    b.truncate(4);
    b[3] = b'A';
    b.extend_from_slice(&[0x31, 0x41, 0x59, 0x26, 0x53, 0x59]);
    b.extend_from_slice(&[0u8; 12]);
    let path = write_case("bz2-lvl", "badlevel.bz", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "BZIP2_BAD_LEVEL");
}

// ---------------------------------------------------------------------------
// zstd
// ---------------------------------------------------------------------------

#[test]
fn zstd_valid_is_ok() {
    let mut b = vec![0x28, 0xB5, 0x2F, 0xFD];
    b.push(0x00); // desc：无 FCS、无单段、无校验和
    b.push(0x00); // Window_Descriptor
    b.extend_from_slice(&[0u8; 4]); // 假帧数据
    let path = write_case("zstd-ok", "ok.zst", &b);
    let r = verify_file(&path, &opts());
    assert_ok(&r, "zstd");
}

#[test]
fn zstd_reserved_bits_is_corrupt() {
    let mut b = vec![0x28, 0xB5, 0x2F, 0xFD];
    b.push(0x18); // bit4/bit3 保留位为 1
    b.extend_from_slice(&[0u8; 8]);
    let path = write_case("zstd-rsv", "rsv.zst", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "ZSTD_BAD_DESCRIPTOR");
}

#[test]
fn zstd_frame_header_beyond_file_is_corrupt() {
    let mut b = vec![0x28, 0xB5, 0x2F, 0xFD];
    b.push(0x20); // 单段 + FCS=0 → 帧头需 1 字节 FCS
    // 文件在描述符后就结束 → 4+2=6 > 5
    let path = write_case("zstd-trunc", "trunc.zst", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "ZSTD_TRUNCATED");
}

// ---------------------------------------------------------------------------
// lz4 frame
// ---------------------------------------------------------------------------

#[test]
fn lz4_valid_is_ok() {
    // FLG=0x40（版本 010）BD=0x10（最大块级别位）HC=任意
    let mut b = vec![0x04, 0x22, 0x4D, 0x18, 0x40, 0x10, 0x78];
    b.extend_from_slice(&[0u8; 8]);
    let path = write_case("lz4-ok", "ok.lz4", &b);
    let r = verify_file(&path, &opts());
    assert_ok(&r, "LZ4");
}

#[test]
fn lz4_bad_version_is_corrupt() {
    let mut b = vec![0x04, 0x22, 0x4D, 0x18];
    b.push(0x60); // 版本位 = 011
    b.push(0x10);
    b.push(0x78);
    b.extend_from_slice(&[0u8; 8]);
    let path = write_case("lz4-ver", "badver.lz4", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "LZ4_BAD_HEADER");
}

#[test]
fn lz4_bad_bd_reserved_is_corrupt() {
    let mut b = vec![0x04, 0x22, 0x4D, 0x18, 0x40, 0x8F, 0x78];
    b.extend_from_slice(&[0u8; 8]);
    let path = write_case("lz4-bd", "badbd.lz4", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "LZ4_BAD_HEADER");
}
