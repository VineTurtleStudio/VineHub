//! kind 判定后置证据：内嵌封面与散图 dHash 交叉比对（需要文件 IO，独立于
//! `rules`/`detect` 纯逻辑层）。`scan_directory` 与 `kind_audit` 在 `detect()` 之后调用。
use crate::detect::{DetectNode, DetectResult};
use crate::meta::embedded_cover;
use crate::scan::rules;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// 参与比对的散图上限（单单位）
const MAX_IMAGES: usize = 16;
/// 单图字节上限（跳过大图）
const MAX_IMAGE_BYTES: u64 = 30 * 1024 * 1024;
/// dHash 汉明距离阈值（64bit，≤10 视为同一图的缩放/再编码）
const HAMMING_MAX: u32 = 10;

/// dHash：缩到 9×8 灰度，同行右移比较 → 64bit 指纹
fn dhash(bytes: &[u8]) -> Option<u64> {
    let img = image::load_from_memory(bytes).ok()?;
    let g = img
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            h <<= 1;
            if g.get_pixel(x, y)[0] > g.get_pixel(x + 1, y)[0] {
                h |= 1;
            }
        }
    }
    Some(h)
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 单位根目录散图与内嵌封面的匹配率（0..=1）。
/// 取不到内嵌封面、无可比散图或图都解码失败 → None。
pub fn cover_match_ratio(unit: &Path) -> Option<f32> {
    let cover = embedded_cover(unit)?;
    let ch = dhash(&cover)?;
    let mut compared = 0u32;
    let mut matched = 0u32;
    for entry in fs::read_dir(unit).ok()?.flatten() {
        if compared as usize >= MAX_IMAGES {
            break;
        }
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext_ok = p
            .extension()
            .map(|e| {
                let s = e.to_string_lossy().to_ascii_lowercase();
                matches!(
                    s.as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" | "tif" | "tiff"
                )
            })
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if len == 0 || len > MAX_IMAGE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&p) else {
            continue;
        };
        let Some(h) = dhash(&bytes) else {
            continue;
        };
        compared += 1;
        if hamming(ch, h) <= HAMMING_MAX {
            matched += 1;
        }
    }
    if compared == 0 {
        return None;
    }
    Some(matched as f32 / compared as f32)
}

/// enrich 执行统计（审计/诊断用）
#[derive(Debug, Default, Clone, Copy)]
pub struct EnrichStats {
    /// 音频+散图单位总数
    pub candidates: u32,
    /// 路径门高置信跳过（不做 IO）
    pub gated: u32,
    /// 实际完成封面比对（cover_match 已写入）
    pub computed: u32,
}

/// 后置封面比对：对「音频 + 散图」且**路径门未命中**的单位（评分层裁决区）补
/// `cover_match` 证据后重判 kind/needs_review，并同步树节点。
/// 散图 ≈ 内嵌封面 → 图片只是专辑封面（专辑加分）；不匹配且图占优 → 真扫描（图册加分）。
pub fn enrich_cover_matches(root: &Path, result: &mut DetectResult) -> EnrichStats {
    let mut stat = EnrichStats::default();
    let mut updates: HashMap<String, (String, bool, Option<f32>)> = HashMap::new();
    for u in result.units.iter_mut() {
        if u.stats.audio_count == 0 || u.stats.image_count == 0 {
            continue;
        }
        stat.candidates += 1;
        // 路径门命中（confidence = 1.0）= 高置信，不做 IO
        if rules::kind_verdict(&u.rel_path, &u.stats).confidence >= 1.0 {
            stat.gated += 1;
            continue;
        }
        let Some(r) = cover_match_ratio(&root.join(&u.rel_path)) else {
            continue;
        };
        stat.computed += 1;
        u.stats.cover_match = Some(r);
        let v = rules::kind_verdict(&u.rel_path, &u.stats);
        u.kind = v.kind.to_string();
        u.needs_review = v.review;
        updates.insert(u.rel_path.clone(), (u.kind.clone(), u.needs_review, Some(r)));
    }
    if !updates.is_empty() {
        sync_tree(&mut result.tree, &updates);
    }
    stat
}

fn sync_tree(nodes: &mut [DetectNode], updates: &HashMap<String, (String, bool, Option<f32>)>) {
    for n in nodes {
        if n.is_unit {
            if let Some((kind, review, r)) = updates.get(&n.rel_path) {
                n.kind = kind.clone();
                n.needs_review = *review;
                if let Some(r) = r {
                    if let Some(st) = n.stats.as_mut() {
                        st.cover_match = Some(*r);
                    }
                }
            }
        } else {
            sync_tree(&mut n.children, updates);
        }
    }
}
