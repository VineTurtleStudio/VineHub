use std::fs;
use std::path::PathBuf;
use vine_press_lib::detect::detect;
use vine_press_lib::kindx::{cover_match_ratio, enrich_cover_matches};
use vine_press_lib::project::Overrides;
use vine_press_lib::scan::scan_tree;

fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("vinepress_kindx_test").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn solid_png(r: u8, g: u8, b: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([r, g, b, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn noisy_jpeg(size: u32) -> Vec<u8> {
    let mut img = image::RgbImage::new(size, size);
    let mut s: u32 = 7;
    for p in img.pixels_mut() {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        p.0 = [(s >> 16) as u8, (s >> 8) as u8, s as u8];
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// 合成带 PICTURE 块的 flac（与 cover_test 同构）
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

#[test]
fn cover_ratio_counts_matching_over_total() {
    let d = fixture("ratio");
    let cover = solid_png(200, 30, 30);
    fs::write(d.join("t.flac"), synth_flac_with_picture(&cover)).unwrap();
    fs::write(d.join("a.png"), &cover).unwrap(); // 与内嵌封面同图 → 匹配
    fs::write(d.join("b.jpg"), noisy_jpeg(64)).unwrap(); // 噪声图 → 不匹配
    let r = cover_match_ratio(&d).expect("应取到比对结果");
    assert!((r - 0.5).abs() < 1e-6, "2 张散图 1 匹配应为 0.5，实为 {}", r);
}

#[test]
fn cover_ratio_none_without_embedded_or_images() {
    let d = fixture("none");
    fs::write(d.join("a.png"), solid_png(1, 2, 3)).unwrap();
    assert!(cover_match_ratio(&d).is_none(), "无内嵌封面应 None");
    let d2 = fixture("none2");
    fs::write(d2.join("t.flac"), synth_flac_with_picture(&solid_png(1, 2, 3))).unwrap();
    assert!(cover_match_ratio(&d2).is_none(), "无散图应 None");
}

/// 散图 = 内嵌封面的「歌曲 + 散图」堆：enrich 前低置信，enrich 后封面证据把专辑判出并解除复核
#[test]
fn enrich_flips_mixed_pile_with_cover_evidence() {
    let root = fixture("flip");
    let unit = root.join("Plain").join("rip");
    fs::create_dir_all(&unit).unwrap();
    let cover = solid_png(180, 60, 90);
    fs::write(unit.join("t.flac"), synth_flac_with_picture(&cover)).unwrap();
    fs::write(unit.join("a.png"), &cover).unwrap();
    fs::write(unit.join("b.png"), &cover).unwrap();
    fs::write(unit.join("c.png"), &cover).unwrap();

    let tree = scan_tree(&root).expect("扫描失败");
    let mut before = detect(&tree, &Overrides::default());
    let u0 = &before.units[0];
    assert_eq!(u0.rel_path, "Plain/rip");
    assert!(u0.stats.cover_match.is_none(), "enrich 前无封面证据");
    assert!(u0.needs_review, "音频/图接近均分应先标复核: {}", u0.kind);

    let stat = enrich_cover_matches(&root, &mut before);
    assert_eq!(stat.candidates, 1, "音频+散图单位应进入候选");
    assert_eq!(stat.gated, 0, "路径无类别词不应被门跳过");
    assert_eq!(stat.computed, 1, "应完成封面比对");

    let u1 = &before.units[0];
    assert_eq!(u1.stats.cover_match, Some(1.0), "散图应全部匹配内嵌封面");
    assert_eq!(u1.kind, "album", "封面证据应判为专辑");
    assert!(!u1.needs_review, "明确结论应解除复核");
    // 树节点同步
    fn find<'a>(
        nodes: &'a [vine_press_lib::detect::DetectNode],
        rel: &str,
    ) -> Option<&'a vine_press_lib::detect::DetectNode> {
        for n in nodes {
            if n.rel_path == rel {
                return Some(n);
            }
            if !n.is_unit {
                if let Some(f) = find(&n.children, rel) {
                    return Some(f);
                }
            }
        }
        None
    }
    assert_eq!(before.tree[0].kind, "group", "顶层是容器 Plain");
    let node = find(&before.tree, "Plain/rip").expect("未找到树节点");
    assert_eq!(node.kind, "album", "树节点 kind 应同步");
    assert!(!node.needs_review, "树节点复核标记应同步");
}
