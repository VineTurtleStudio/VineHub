use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-image-{}-{}",
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

fn assert_corrupt_any(r: &Report, accepted: &[&str]) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs = codes(r);
    assert!(
        cs.iter().any(|c| accepted.contains(c)),
        "findings: {:?}，期望其中之一 {accepted:?}",
        r.findings
    );
}

fn encoded(kind: image::ImageFormat) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([
            ((x * 7) % 256) as u8,
            ((y * 13) % 256) as u8,
            ((x + y) % 256) as u8,
        ])
    }));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, kind).unwrap();
    buf.into_inner()
}

// ---------------------------------------------------------------------------
// GIF
// ---------------------------------------------------------------------------

#[test]
fn gif_valid_is_ok() {
    let path = write_case("gif-ok", "ok.gif", &encoded(image::ImageFormat::Gif));
    let r = verify_file(&path, &opts());
    assert_ok(&r, "GIF");
}

#[test]
fn gif_truncated_is_corrupt() {
    let data = encoded(image::ImageFormat::Gif);
    let path = write_case("gif-trunc", "trunc.gif", &data[..data.len() - 10]);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["GIF_NO_TRAILER", "GIF_TRUNCATED"]);
}

#[test]
fn gif_bad_block_is_corrupt() {
    let mut data = encoded(image::ImageFormat::Gif);
    let packed = data[10];
    let gct = 3usize * (1usize << ((packed & 0x07) + 1));
    let block = 13 + gct; // 全局颜色表之后的第一个块标识
    assert!(block < data.len(), "生成的 GIF 结构异常");
    data[block] = 0x99;
    let path = write_case("gif-block", "bad-block.gif", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["GIF_BAD_BLOCK"]);
}

// ---------------------------------------------------------------------------
// BMP
// ---------------------------------------------------------------------------

#[test]
fn bmp_valid_is_ok() {
    let path = write_case("bmp-ok", "ok.bmp", &encoded(image::ImageFormat::Bmp));
    let r = verify_file(&path, &opts());
    assert_ok(&r, "BMP");
}

#[test]
fn bmp_truncated_is_corrupt() {
    let data = encoded(image::ImageFormat::Bmp);
    let path = write_case("bmp-trunc", "trunc.bmp", &data[..data.len() - 40]);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["BMP_TRUNCATED"]);
}

#[test]
fn bmp_bad_signature_is_corrupt() {
    let mut data = encoded(image::ImageFormat::Bmp);
    data[0] = b'Z';
    data[1] = b'Z';
    let path = write_case("bmp-sig", "bad-sig.bmp", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["BMP_BAD_SIG"]);
}

#[test]
fn bmp_bad_bpp_is_corrupt() {
    let mut data = encoded(image::ImageFormat::Bmp);
    data[28] = 7; // 每像素位数
    let path = write_case("bmp-bpp", "bad-bpp.bmp", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["BMP_BAD_BPP"]);
}

// ---------------------------------------------------------------------------
// WebP
// ---------------------------------------------------------------------------

fn encoded_webp() -> Vec<u8> {
    let mut out = Vec::new();
    let pixels: Vec<u8> = (0..32u32 * 32 * 3)
        .map(|i| (i % 251) as u8)
        .collect();
    image_webp::WebPEncoder::new(&mut out)
        .encode(&pixels, 32, 32, image_webp::ColorType::Rgb8)
        .expect("WebP 编码失败");
    out
}

#[test]
fn webp_valid_is_ok() {
    let path = write_case("webp-ok", "ok.webp", &encoded_webp());
    let r = verify_file(&path, &opts());
    assert_ok(&r, "WebP");
}

#[test]
fn webp_truncated_is_corrupt() {
    let data = encoded_webp();
    let path = write_case("webp-trunc", "trunc.webp", &data[..data.len() - 20]);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["WEBP_TRUNCATED"]);
}

#[test]
fn webp_bad_first_chunk_is_corrupt() {
    let mut data = encoded_webp();
    data[12..16].copy_from_slice(b"JUNK");
    let path = write_case("webp-chunk", "bad-chunk.webp", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["WEBP_BAD_CHUNK"]);
}

// ---------------------------------------------------------------------------
// TIFF
// ---------------------------------------------------------------------------

#[test]
fn tiff_valid_is_ok() {
    let path = write_case("tiff-ok", "ok.tif", &encoded(image::ImageFormat::Tiff));
    let r = verify_file(&path, &opts());
    assert_ok(&r, "TIFF");
}

#[test]
fn tiff_truncated_is_corrupt() {
    let data = encoded(image::ImageFormat::Tiff);
    let path = write_case("tiff-trunc", "trunc.tif", &data[..data.len() / 2]);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["TIFF_TRUNCATED", "TIFF_BAD_OFFSET", "TIFF_NO_IFD"]);
}

#[test]
fn tiff_bad_strip_offset_is_corrupt() {
    let mut data = encoded(image::ImageFormat::Tiff);
    let le = data[0] == b'I' && data[1] == b'I';
    let rd16 = |d: &[u8], o: usize| -> u16 {
        let b: [u8; 2] = d[o..o + 2].try_into().unwrap();
        if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    };
    let rd32 = |d: &[u8], o: usize| -> u32 {
        let b: [u8; 4] = d[o..o + 4].try_into().unwrap();
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };

    let ifd = rd32(&data, 4) as usize;
    let n = rd16(&data, ifd);
    let mut patched = false;
    for k in 0..n {
        let p = ifd + 2 + k as usize * 12;
        if rd16(&data, p) == 273 {
            // StripOffsets：改成指向文件之外的偏移
            let huge = u32::MAX;
            let bytes: [u8; 4] = if le { huge.to_le_bytes() } else { huge.to_be_bytes() };
            data[p + 8..p + 12].copy_from_slice(&bytes);
            patched = true;
            break;
        }
    }
    assert!(patched, "生成的 TIFF 缺少 StripOffsets 条目");

    let path = write_case("tiff-strip", "bad-strip.tif", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["TIFF_BAD_OFFSET"]);
}

#[test]
fn tiff_bad_signature_is_corrupt() {
    let mut data = encoded(image::ImageFormat::Tiff);
    data[0] = b'X';
    data[1] = b'X';
    let path = write_case("tiff-sig", "bad-sig.tif", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt_any(&r, &["TIFF_BAD_SIG"]);
}
