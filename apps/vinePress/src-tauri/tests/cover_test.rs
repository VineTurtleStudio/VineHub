use std::fs;
use std::path::PathBuf;
use vine_press_lib::commands;
use vine_press_lib::meta::cover_bytes;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("vinepress_cover_test").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn png_1x1() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn jpeg_of(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 120, 200]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// 噪声图：字节数显著大于纯色图，保证「最大图片」判定稳定
fn jpeg_noisy(size: u32) -> Vec<u8> {
    let mut img = image::RgbImage::new(size, size);
    let mut s: u32 = 42;
    for p in img.pixels_mut() {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        p.0 = [(s >> 16) as u8, (s >> 8) as u8, s as u8];
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

fn is_png(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
}

fn is_jpeg(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF
}

fn synth_flac_with_picture(png: &[u8]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.push(0x00);
    out.extend_from_slice(&[0x00, 0x00, 34]);
    out.extend_from_slice(&[0u8; 34]);
    let mut pic = Vec::new();
    pic.extend_from_slice(&3u32.to_be_bytes());
    pic.extend_from_slice(&9u32.to_be_bytes());
    pic.extend_from_slice(b"image/png");
    pic.extend_from_slice(&0u32.to_be_bytes());
    pic.extend_from_slice(&1u32.to_be_bytes());
    pic.extend_from_slice(&1u32.to_be_bytes());
    pic.extend_from_slice(&24u32.to_be_bytes());
    pic.extend_from_slice(&0u32.to_be_bytes());
    pic.extend_from_slice(&(png.len() as u32).to_be_bytes());
    pic.extend_from_slice(png);
    out.push(0x86);
    let n = pic.len();
    out.extend_from_slice(&[((n >> 16) & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, (n & 0xFF) as u8]);
    out.extend_from_slice(&pic);
    out
}

fn synth_mp3_with_apic(png: &[u8]) -> Vec<u8> {
    let mut frame_body = vec![0u8];
    frame_body.extend_from_slice(b"image/png\0");
    frame_body.push(3);
    frame_body.push(0);
    frame_body.extend_from_slice(png);
    let mut frame = Vec::new();
    frame.extend_from_slice(b"APIC");
    frame.extend_from_slice(&(frame_body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&frame_body);
    let tag_size = frame.len() as u32;
    let mut tag = Vec::new();
    tag.extend_from_slice(b"ID3");
    tag.extend_from_slice(&[3, 0, 0]);
    tag.extend_from_slice(&[
        ((tag_size >> 21) & 0x7F) as u8,
        ((tag_size >> 14) & 0x7F) as u8,
        ((tag_size >> 7) & 0x7F) as u8,
        (tag_size & 0x7F) as u8,
    ]);
    tag.extend_from_slice(&frame);
    tag.extend_from_slice(b"AUDIODATA");
    tag
}

#[test]
fn named_cover_wins_and_case_insensitive() {
    let d = fixture("named");
    fs::write(d.join("COVER.JPG"), jpeg_of(2, 2)).unwrap();
    fs::write(d.join("folder.png"), png_1x1()).unwrap();
    let (mime, bytes) = cover_bytes(&d).expect("应命中命名图");
    assert_eq!(mime, "image/jpeg");
    assert!(is_jpeg(&bytes), "应取到 cover.jpg 而非 folder.png");
}

#[test]
fn largest_image_fallback() {
    let d = fixture("largest");
    fs::write(d.join("a.jpg"), jpeg_of(1, 1)).unwrap();
    fs::write(d.join("b.png"), png_1x1()).unwrap();
    fs::write(d.join("c.jpg"), jpeg_noisy(128)).unwrap();
    fs::write(d.join("note.txt"), b"hello").unwrap();
    let (mime, bytes) = cover_bytes(&d).expect("应取最大图片");
    assert_eq!(mime, "image/jpeg");
    assert!(is_jpeg(&bytes));
    assert_eq!(bytes, jpeg_noisy(128));
}

#[test]
fn embedded_flac_picture_preferred_over_files() {
    let d = fixture("flac_embed");
    fs::write(d.join("scan.jpg"), jpeg_of(8, 8)).unwrap();
    fs::write(d.join("track.flac"), synth_flac_with_picture(&png_1x1())).unwrap();
    let (mime, bytes) = cover_bytes(&d).expect("应取内嵌封面");
    assert_eq!(mime, "image/png");
    assert!(is_png(&bytes), "FLAC PICTURE 应优先于目录内图片");
}

#[test]
fn embedded_id3_apic_extracted() {
    let d = fixture("mp3_apic");
    fs::write(d.join("t.mp3"), synth_mp3_with_apic(&png_1x1())).unwrap();
    let (mime, bytes) = cover_bytes(&d).expect("应取 APIC 内嵌封面");
    assert_eq!(mime, "image/png");
    assert!(is_png(&bytes));
    assert_eq!(bytes, png_1x1());
}

#[test]
fn no_images_returns_none() {
    let d = fixture("none");
    fs::write(d.join("readme.txt"), b"nothing").unwrap();
    fs::write(d.join("track.flac"), b"fLaC\0\0\0\x22\0\0\0\0\0\0\0\0").unwrap();
    assert!(cover_bytes(&d).is_none());
}

#[test]
fn command_returns_data_url_and_rejects_traversal() {
    let d = fixture("cmd");
    let unit = d.join("album");
    fs::create_dir_all(&unit).unwrap();
    fs::write(unit.join("cover.png"), png_1x1()).unwrap();
    let root = d.to_string_lossy().to_string();
    let url = commands::cover_image(root.clone(), "album".into())
        .unwrap()
        .expect("应返回封面");
    assert!(url.starts_with("data:image/jpeg;base64,"), "应为缩略 JPEG: {}", &url[..url.len().min(40)]);
    let none = commands::cover_image(root.clone(), "missing".into());
    assert!(none.is_err(), "不存在的目录应报错");
    let bad = commands::cover_image(root, "../outside".into());
    assert!(bad.is_err(), "路径穿越应报错");
}
