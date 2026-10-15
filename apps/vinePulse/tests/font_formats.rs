use std::fs;
use std::path::PathBuf;

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-font-{}-{}",
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

fn assert_corrupt(r: &Report, code: &str) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

fn assert_warning(r: &Report, code: &str) {
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
    assert_ne!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
}

/// sfnt 表求和校验和（大端 u32 累加，尾部补 0）
fn sfnt_sum(d: &[u8]) -> u32 {
    let mut s = 0u32;
    for c in d.chunks(4) {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        s = s.wrapping_add(u32::from_be_bytes(b));
    }
    s
}

/// 构造最小 TrueType：12 字节头 + 表目录 + 连续表数据
fn sfnt(tables: &[(&[u8; 4], Vec<u8>, u32)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let base = 12 + 16 * n as usize;
    let mut dir = Vec::new();
    let mut data = Vec::new();
    for (tag, payload, cs) in tables {
        dir.extend_from_slice(*tag);
        dir.extend_from_slice(&cs.to_be_bytes());
        dir.extend_from_slice(&((base + data.len()) as u32).to_be_bytes());
        dir.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        data.extend_from_slice(payload);
    }
    let mut out = vec![0u8; 12];
    out[0..4].copy_from_slice(&[0x00, 0x01, 0x00, 0x00]);
    out[4..6].copy_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&data);
    out
}

#[test]
fn ttf_valid_is_ok() {
    let cmap = vec![0x00u8; 20];
    let head = vec![0x11u8; 54];
    let data = sfnt(&[
        (b"cmap", cmap.clone(), sfnt_sum(&cmap)),
        (b"head", head.clone(), sfnt_sum(&head)),
    ]);
    let path = write_case("ttf-ok", "ok.ttf", &data);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Font");
}

#[test]
fn oto_otto_is_opentype() {
    let cff = vec![0x22u8; 16];
    let data = sfnt(&[(b"CFF ", cff.clone(), sfnt_sum(&cff))]);
    let mut data = data;
    data[0..4].copy_from_slice(b"OTTO");
    let path = write_case("otf-ok", "ok.otf", &data);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Font");
}

#[test]
fn ttf_table_out_of_range_is_corrupt() {
    // 目录项声明 length 远超文件
    let mut data = sfnt(&[(b"glyf", vec![0u8; 8], 0)]);
    let dir_at = 12 + 8; // tag(4)+checksum(4) → offset 字段
    data[dir_at + 4..dir_at + 8].copy_from_slice(&0xFFFFu32.to_be_bytes()); // offset
    data[dir_at + 8..dir_at + 12].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes()); // length
    let path = write_case("ttf-oob", "oob.ttf", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FONT_TABLE_OUT_OF_RANGE");
}

#[test]
fn ttf_zero_tables_is_corrupt() {
    let mut data = vec![0u8; 12];
    data[0..4].copy_from_slice(&[0x00, 0x01, 0x00, 0x00]);
    // numTables = 0
    let path = write_case("ttf-empty", "empty.ttf", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FONT_NO_TABLES");
}

#[test]
fn ttf_bad_checksum_is_warning() {
    let cmap = vec![0u8; 20];
    // 故意给一个非零但错误的校验和
    let data = sfnt(&[(b"cmap", cmap.clone(), 0x1122_3344)]);
    let path = write_case("ttf-cs", "cs.ttf", &data);
    let r = verify_file(&path, &opts());
    assert_warning(&r, "FONT_TABLE_CHECKSUM_MISMATCH");
}

#[test]
fn woff_bad_length_is_corrupt() {
    let mut b = vec![0u8; 64];
    b[0..4].copy_from_slice(b"wOFF");
    b[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // flavor
    b[8..12].copy_from_slice(&999u32.to_be_bytes()); // length ≠ 文件 64
    b[12..14].copy_from_slice(&1u16.to_be_bytes()); // numTables
    let path = write_case("woff-len", "bad.woff", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FONT_BAD_LENGTH");
}

#[test]
fn woff2_valid_is_ok() {
    let mut b = vec![0u8; 200];
    b[0..4].copy_from_slice(b"wOF2");
    b[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // flavor
    b[8..12].copy_from_slice(&200u32.to_be_bytes()); // length = 文件大小
    b[12..14].copy_from_slice(&3u16.to_be_bytes()); // numTables
    b[16..20].copy_from_slice(&96u32.to_be_bytes()); // totalCompressedSize（48+96 ≤ 200）
    let path = write_case("woff2-ok", "ok.woff2", &b);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Font");
}

#[test]
fn woff2_compressed_data_beyond_file_is_corrupt() {
    let mut b = vec![0u8; 64];
    b[0..4].copy_from_slice(b"wOF2");
    b[8..12].copy_from_slice(&64u32.to_be_bytes());
    b[12..14].copy_from_slice(&1u16.to_be_bytes());
    b[16..20].copy_from_slice(&1000u32.to_be_bytes()); // 48+1000 > 64
    let path = write_case("woff2-oob", "oob.woff2", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FONT_DATA_OUT_OF_RANGE");
}

#[test]
fn ttc_bad_sfnt_offset_is_corrupt() {
    let mut b = vec![0u8; 24];
    b[0..4].copy_from_slice(b"ttcf");
    b[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // version
    b[8..12].copy_from_slice(&1u32.to_be_bytes()); // numFonts
    b[12..16].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes()); // 偏移指向文件外
    let path = write_case("ttc-oob", "oob.ttc", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FONT_TTC_BAD_OFFSET");
}

#[test]
fn ttc_valid_collection_is_ok() {
    let cmap = vec![0u8; 16];
    let inner = sfnt(&[(b"cmap", cmap.clone(), sfnt_sum(&cmap))]);
    let mut b = vec![0u8; 12 + 4];
    b[0..4].copy_from_slice(b"ttcf");
    b[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    b[8..12].copy_from_slice(&1u32.to_be_bytes());
    b[12..16].copy_from_slice(&((12 + 4) as u32).to_be_bytes()); // 内部 sfnt 紧随偏移表
    b.extend_from_slice(&inner);
    let path = write_case("ttc-ok", "ok.ttc", &b);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Font");
}
