use crate::project::Entry;
use crate::scan::UnitStats;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const NOTE_FOOTER: &str = "本信息文件由本地";

/// 生成说明文件所需的上下文
pub struct NoteInput<'a> {
    pub entry: &'a Entry,
    pub stats: &'a UnitStats,
    pub unit_path: &'a Path,
}

pub fn format_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * MIB;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else {
        format!("{:.2} MiB", b / MIB)
    }
}

// ---------------------------------------------------------------- 探测

struct Probe {
    /// 全部内容文件（相对单位路径）
    content_files: Vec<String>,
    content_size: u64,
    images: Vec<String>,
    audios: Vec<String>,
    cues: Vec<String>,
    /// 一级子目录 -> 内容文件数
    subdir_content: HashMap<String, u32>,
}

fn walk(dir: &Path, rel: &str, p: &mut Probe) {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let mut sub: Vec<(String, PathBuf)> = Vec::new();
    let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            sub.push((name, e.path()));
        } else if meta.is_file() {
            files.push((name, e.path(), meta.len()));
        }
    }
    sub.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let first_seg = rel.split('/').next().unwrap_or("").to_string();
    for (name, _path, size) in files {
        let rel_file = if rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel, name)
        };
        let ext = crate::scan::rules::extension_of(&name).to_ascii_lowercase();
        let is_content = crate::scan::rules::is_content_file(&name);
        let is_img = matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff"
        );
        let is_audio = matches!(
            ext.as_str(),
            "flac" | "mp3" | "wav" | "ape" | "wv" | "m4a" | "m4b" | "ogg" | "aac" | "opus"
        );
        if is_content {
            p.content_files.push(rel_file.clone());
            p.content_size += size;
            if !first_seg.is_empty() {
                *p.subdir_content.entry(first_seg.clone()).or_insert(0) += 1;
            }
            if is_img {
                p.images.push(rel_file.clone());
            }
            if is_audio {
                p.audios.push(rel_file.clone());
            }
        }
        if ext == "cue" {
            p.cues.push(rel_file);
        }
    }
    for (name, path) in sub {
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel, name)
        };
        walk(&path, &child_rel, p);
    }
}

fn probe(unit_path: &Path) -> Probe {
    let mut p = Probe {
        content_files: Vec::new(),
        content_size: 0,
        images: Vec::new(),
        audios: Vec::new(),
        cues: Vec::new(),
        subdir_content: HashMap::new(),
    };
    walk(unit_path, "", &mut p);
    p
}

/// 读取图像尺寸（只读文件头，抽样最多 60 张）
fn sample_resolutions(unit_path: &Path, images: &[String]) -> (Vec<((u32, u32), u32)>, usize, usize) {
    let total = images.len();
    let take = total.min(60);
    let mut counts: Vec<((u32, u32), u32)> = Vec::new();
    let mut sampled = 0usize;
    for rel in images.iter().take(take) {
        let path = unit_path.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Ok((w, h)) = image::image_dimensions(&path) {
            sampled += 1;
            match counts.iter_mut().find(|((cw, ch), _)| *cw == w && *ch == h) {
                Some(c) => c.1 += 1,
                None => counts.push(((w, h), 1)),
            }
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    (counts, sampled, total)
}

struct AudioInfo {
    format: String,
    rate: Option<u32>,
    bits: Option<u32>,
}

fn probe_audio(unit_path: &Path, audios: &[String]) -> Vec<AudioInfo> {
    let mut out: Vec<AudioInfo> = Vec::new();
    for rel in audios.iter().take(8) {
        let path = unit_path.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let ext = crate::scan::rules::extension_of(rel).to_ascii_lowercase();
        let info = match ext.as_str() {
            "wav" => probe_wav(&path),
            "flac" => probe_flac(&path),
            _ => None,
        };
        let info = info.unwrap_or(AudioInfo {
            format: ext.to_uppercase(),
            rate: None,
            bits: None,
        });
        if !out.iter().any(|i| i.format == info.format) {
            out.push(info);
        }
    }
    out
}

fn probe_wav(path: &Path) -> Option<AudioInfo> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    if f.read_exact(&mut head).is_err() || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    // 只按需读取块头与 fmt 块（12 + 8 + 16 字节），不把整文件读进内存
    let mut pos = 12u64;
    for _ in 0..64 {
        if f.seek(SeekFrom::Start(pos)).is_err() {
            return None;
        }
        let mut chunk = [0u8; 8];
        if f.read_exact(&mut chunk).is_err() {
            return None;
        }
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        if &chunk[0..4] == b"fmt " {
            let mut fmt = [0u8; 16];
            if f.read_exact(&mut fmt).is_err() {
                return None;
            }
            let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
            let _channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            return Some(AudioInfo {
                format: "WAV（无损）".into(),
                rate: Some(rate),
                bits: Some(bits as u32),
            });
        }
        pos += 8 + size + (size & 1);
    }
    None
}

fn probe_flac(path: &Path) -> Option<AudioInfo> {
    let mut buf = [0u8; 64];
    let mut f = fs::File::open(path).ok()?;
    use std::io::Read;
    let n = f.read(&mut buf).ok()?;
    if n < 42 || &buf[0..4] != b"fLaC" {
        return None;
    }
    // STREAMINFO: 4 header bytes + 34 bytes
    let b = &buf[8..42];
    let rate = ((b[10] as u32) << 12) | ((b[11] as u32) << 4) | ((b[12] as u32) >> 4);
    let bits = (((b[12] as u32) & 0x0f) << 1) + ((b[13] as u32) >> 7) + 1;
    Some(AudioInfo {
        format: "FLAC（无损）".into(),
        rate: Some(rate),
        bits: Some(bits),
    })
}

fn read_text(path: &Path) -> String {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).to_string();
    }
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return s.to_string();
    }
    let (s, _, _) = encoding_rs::GBK.decode(&bytes);
    s.into_owned()
}

/// 解析 cue 的曲目名
pub fn parse_cue(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = false;
    for line in text.lines() {
        let l = line.trim();
        if l.to_ascii_uppercase().starts_with("TRACK ") {
            pending = true;
            continue;
        }
        if pending {
            if let Some(rest) = strip_field(l, "TITLE") {
                out.push(rest);
                pending = false;
            }
        }
    }
    out
}

fn strip_field(line: &str, key: &str) -> Option<String> {
    if line.len() < key.len() || !line[..key.len()].eq_ignore_ascii_case(key) {
        return None;
    }
    let rest = line[key.len()..].trim_start();
    // 支持 `KEY=value` 与 CUE 的 `KEY "value"` 两种写法
    let rest = match rest.strip_prefix('=') {
        Some(r) => r.trim_start(),
        None => rest,
    };
    let rest = rest.trim_matches('"').trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn stem_of(name: &str) -> String {
    let stem = match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    };
    let mut s = stem.trim().to_string();
    // 去掉前导曲目号
    let trimmed = s.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '_' || c == ' ');
    if !trimmed.is_empty() && trimmed.len() < s.len() {
        s = trimmed.to_string();
    }
    s
}

/// 单位整体的曲目列表（cue 优先，其次音频文件名）
fn unit_tracks(unit_path: &Path, p: &Probe) -> Vec<String> {
    if let Some(cue) = p.cues.iter().find(|c| !c.contains('/')) {
        let text = read_text(&unit_path.join(cue.replace('/', std::path::MAIN_SEPARATOR_STR)));
        let t = parse_cue(&text);
        if !t.is_empty() {
            return t;
        }
    }
    if let Some(cue) = p.cues.first() {
        let text = read_text(&unit_path.join(cue.replace('/', std::path::MAIN_SEPARATOR_STR)));
        let t = parse_cue(&text);
        if !t.is_empty() {
            return t;
        }
    }
    p.audios
        .iter()
        .map(|a| stem_of(a.rsplit('/').next().unwrap_or(a)))
        .collect()
}

/// 各子目录（碟片）的曲目列表
fn subdir_tracks(unit_path: &Path, p: &Probe, sub: &str) -> Vec<String> {
    let prefix = format!("{}/", sub);
    let mut cue = p
        .cues
        .iter()
        .find(|c| c.starts_with(&prefix) && !c[prefix.len()..].contains('/'));
    if cue.is_none() {
        cue = p.cues.iter().find(|c| c.starts_with(&prefix));
    }
    if let Some(c) = cue {
        let text = read_text(&unit_path.join(c.replace('/', std::path::MAIN_SEPARATOR_STR)));
        let t = parse_cue(&text);
        if !t.is_empty() {
            return t;
        }
    }
    p.audios
        .iter()
        .filter(|a| a.starts_with(&prefix))
        .map(|a| stem_of(a.rsplit('/').next().unwrap_or(a)))
        .collect()
}

// ---------------------------------------------------------------- 渲染

pub fn render_note(input: &NoteInput) -> String {
    let p = probe(input.unit_path);
    let e = input.entry;
    let s = input.stats;

    let volumes: Vec<&crate::scan::SubdirStat> = s
        .subdirs
        .iter()
        .filter(|d| crate::scan::rules::is_part_name(&d.name))
        .collect();
    let is_volume_unit = volumes.len() >= 2;

    let albumish = s.audio_count > 0 || e.kind == "album";
    if albumish && is_volume_unit {
        render_multi(input, &p, &volumes)
    } else if albumish {
        render_album(input, &p)
    } else {
        render_publication(input, &p)
    }
}

fn render_album(input: &NoteInput, p: &Probe) -> String {
    let e = input.entry;
    let tracks = if e.tracklist.is_empty() {
        unit_tracks(input.unit_path, p)
    } else {
        e.tracklist.clone()
    };
    let audio_desc = audio_format_line(input.unit_path, p);
    let mut out = String::new();
    out.push_str(&banner("  专辑信息 （Album Info）"));
    out.push_str("\n\n");
    out.push_str(&field("专辑名称", pick(&e.title, &e.name)));
    out.push_str(&field("艺人", pick(&e.artist, "本地资料未记录")));
    out.push_str(&field("发行日期", pick(&e.date, "本地资料未记录")));
    out.push_str(&field("唱片公司", pick(&e.label, "本地资料未记录")));
    out.push_str(&field("规格品番", pick(&e.catalog, "本地资料未记录")));
    if !e.rank.is_empty() {
        out.push_str(&field("最高位", &e.rank));
    }
    if !e.notes.is_empty() {
        out.push_str(&field("备注", &e.notes));
    }
    out.push_str(&field("文件格式", &audio_desc));
    out.push_str(&field("曲目数量", &format!("{} 首", tracks.len())));
    out.push_str("\n【曲目列表 / Tracklist】\n");
    if tracks.is_empty() {
        out.push_str("  （未检测到曲目信息）\n");
    } else {
        for (i, t) in tracks.iter().enumerate() {
            out.push_str(&format!("{:2}. {}\n", i + 1, t));
        }
    }
    out.push_str(&footer("本信息文件由本地曲库自动整理生成（曲目以文件夹实际内容为准）"));
    out
}

fn render_multi(input: &NoteInput, p: &Probe, volumes: &[&crate::scan::SubdirStat]) -> String {
    let e = input.entry;
    let s = input.stats;
    let audio_desc = audio_format_line(input.unit_path, p);
    let names: Vec<String> = volumes.iter().map(|d| d.name.clone()).collect();
    let mut out = String::new();
    out.push_str(&banner("  作品信息 （Album / Single Info）"));
    out.push_str("\n\n");
    out.push_str(&field("专辑名称", pick(&e.title, &e.name)));
    out.push_str(&field("艺人", pick(&e.artist, "本地资料未记录")));
    out.push_str(&field("发行日期", pick(&e.date, "本地资料未记录")));
    out.push_str(&field("唱片公司", pick(&e.label, "本地资料未记录")));
    out.push_str(&field("规格品番", pick(&e.catalog, "本地资料未记录")));
    if !e.rank.is_empty() {
        out.push_str(&field("最高位", &e.rank));
    }
    if !e.notes.is_empty() {
        out.push_str(&field("备注", &e.notes));
    }
    out.push_str(&field("文件格式", &audio_desc));
    out.push_str(&field("本资源收录", &names.join("、")));
    out.push_str("\n【收录曲目 / Tracklist】\n");
    for name in &names {
        out.push_str(&format!("\n◆ {}\n", name));
        let tracks = subdir_tracks(input.unit_path, p, name);
        if tracks.is_empty() {
            out.push_str("    （未检测到曲目信息）\n");
        } else {
            for (i, t) in tracks.iter().enumerate() {
                out.push_str(&format!("{:4}. {}\n", i + 1, t));
            }
        }
    }
    out.push_str(&footer("本信息文件由本地曲库自动整理生成（曲目以文件夹实际内容为准）"));
    let _ = s;
    out
}

fn render_publication(input: &NoteInput, p: &Probe) -> String {
    let e = input.entry;
    let s = input.stats;
    let mut out = String::new();
    out.push_str(&banner("  资料信息 （Publication Info）"));
    out.push_str("\n\n");
    out.push_str(&field("作品名称", pick(&e.title, &e.name)));
    out.push_str(&field("所属作品", pick(&e.work, "本地资料未记录")));
    out.push_str(&field("分类", pick(&e.category, "本地资料未记录")));
    if !e.notes.is_empty() {
        out.push_str(&field("备注", &e.notes));
    }
    out.push_str(&field("资料类型", &publication_type(e, s)));
    out.push_str(&field(
        "出版方／发行商",
        pick(&e.publisher, &default_publisher(e)),
    ));
    out.push_str(&field(
        "发行日期",
        if e.date.is_empty() {
            default_date(e)
        } else {
            e.date.clone()
        },
    ));
    out.push_str(&field("文件格式", &format_counts(p)));
    out.push_str(&field(
        "分辨率统计",
        &resolution_line(input.unit_path, p),
    ));
    out.push_str(&field(
        "文件数量",
        &format!(
            "{} 个（图像 {} / 文档 {}{}），合计 {}",
            content_count(p),
            s.image_count,
            s.doc_count,
            other_count_suffix(s),
            format_size(p.content_size)
        ),
    ));
    out.push_str(&field("册数／卷数", &volume_count_line(s)));
    out.push_str("\n【收录内容 / Contents】\n");
    out.push_str(&contents_block(input, p));
    out.push_str(&footer("本信息文件由本地资料库自动整理生成（内容以文件夹实际文件为准）"));
    out
}

fn banner(title: &str) -> String {
    format!(
        "==========================================================\n{}\n==========================================================\n",
        title
    )
}

fn field(key: &str, value: impl std::fmt::Display) -> String {
    format!("【{}】{}\n", key, value)
}

fn footer(text: &str) -> String {
    format!(
        "\n----------------------------------------------------------\n  {}\n==========================================================\n",
        text
    )
}

fn pick(value: &str, fallback: &str) -> String {
    let v = value.trim();
    if v.is_empty() {
        fallback.to_string()
    } else {
        v.to_string()
    }
}

fn content_count(p: &Probe) -> usize {
    p.content_files.len()
}

fn other_count_suffix(s: &UnitStats) -> String {
    let mut parts = String::new();
    if s.audio_count > 0 {
        parts.push_str(&format!(" / 音频 {}", s.audio_count));
    }
    if s.video_count > 0 {
        parts.push_str(&format!(" / 视频 {}", s.video_count));
    }
    if s.archive_count > 0 {
        parts.push_str(&format!(" / 压缩包 {}", s.archive_count));
    }
    parts
}

fn publication_type(e: &Entry, s: &UnitStats) -> String {
    let cat = &e.category;
    if cat.contains("画集") {
        return "画集／美术设定集".into();
    }
    if cat == "CG" || cat.eq_ignore_ascii_case("CG") {
        return "游戏 CG 素材（图片）".into();
    }
    if cat.contains("同人誌") {
        return "同人誌".into();
    }
    if cat.contains("漫画") {
        return "漫画".into();
    }
    if cat.contains("小说") {
        return "小说／文本资料".into();
    }
    if cat.contains("卡片") {
        return "卡片收藏".into();
    }
    if cat.contains("壁纸") {
        return "壁纸".into();
    }
    if cat.contains("乐谱") {
        return "乐谱".into();
    }
    // 路径分类缺失时按条目类型兜底（9 类里非专辑的走此模板）
    match e.kind.as_str() {
        "novel" => return "小说／文本资料".into(),
        "comic" => return "漫画".into(),
        "score" => return "乐谱".into(),
        "concert" => return "演唱会／公演影像".into(),
        "video" => return "视频影像".into(),
        "document" => return "文档资料".into(),
        _ => {}
    }
    if s.image_count > 0 && s.doc_count == 0 {
        return "图像资料".into();
    }
    if s.doc_count > 0 && s.image_count == 0 {
        return "文档资料".into();
    }
    "资料".into()
}

fn default_publisher(e: &Entry) -> String {
    if e.work.contains("官方作品") || e.category.contains("官方") {
        return "VisualArt's, Inc.／Key".into();
    }
    if e.category.contains("同人") || e.work.contains("画师") {
        return "非官方同人出版物（本地资料未记录原发行方）".into();
    }
    "本地资料未记录".into()
}

fn default_date(e: &Entry) -> String {
    let year = first_year(&e.work).or_else(|| first_year(&e.name));
    match year {
        Some(y) => format!("本地资料未记录确切日期（作品年份：{}）", y),
        None => "本地资料未记录".into(),
    }
}

fn first_year(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let y: u32 = s[i..i + 4].parse().ok()?;
            if (1970..=2099).contains(&y) {
                return Some(y.to_string());
            }
        }
    }
    None
}

fn format_counts(p: &Probe) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for f in &p.content_files {
        let ext = crate::scan::rules::extension_of(f.rsplit('/').next().unwrap_or(f))
            .to_ascii_uppercase();
        let label = match ext.as_str() {
            "JPG" | "JPEG" | "JFIF" => "JPEG".to_string(),
            "TIF" | "TIFF" => "TIFF".to_string(),
            other => other.to_string(),
        };
        match counts.iter_mut().find(|(l, _)| *l == label) {
            Some(c) => c.1 += 1,
            None => counts.push((label, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let text = counts
        .iter()
        .take(3)
        .map(|(l, c)| format!("{}（{}）", l, c))
        .collect::<Vec<_>>()
        .join("、");
    if text.is_empty() {
        "本地资料未记录".into()
    } else {
        text
    }
}

fn resolution_line(unit_path: &Path, p: &Probe) -> String {
    if p.images.is_empty() {
        return "本地资料未记录".into();
    }
    let (counts, sampled, total) = sample_resolutions(unit_path, &p.images);
    if counts.is_empty() {
        return "本地资料未记录".into();
    }
    let unique = counts.len();
    let shown: Vec<String> = counts
        .iter()
        .take(4)
        .map(|((w, h), c)| format!("{}x{} x{}", w, h, c))
        .collect();
    let mut line = shown.join(" / ");
    if unique > 4 {
        line.push_str(" / ……");
    }
    if unique > 1 {
        line.push_str(&format!(" 共 {} 种分辨率", unique));
    }
    if sampled < total {
        line.push_str(&format!("（抽样 {} 张（共 {} 张））", sampled, total));
    } else {
        line.push_str(&format!("（抽样 {} 张）", sampled));
    }
    line
}

fn volume_count_line(s: &UnitStats) -> String {
    let volumes: Vec<&crate::scan::SubdirStat> = s
        .subdirs
        .iter()
        .filter(|d| crate::scan::rules::is_part_name(&d.name))
        .collect();
    if volumes.len() >= 2 {
        format!("{} 卷", volumes.len())
    } else {
        "单册".into()
    }
}

fn contents_block(input: &NoteInput, p: &Probe) -> String {
    let s = input.stats;
    let volumes: Vec<&crate::scan::SubdirStat> = s
        .subdirs
        .iter()
        .filter(|d| crate::scan::rules::is_part_name(&d.name))
        .collect();
    let mut out = String::new();
    if volumes.len() >= 2 {
        let mut sum = 0u32;
        for (i, d) in volumes.iter().enumerate() {
            let n = p.subdir_content.get(&d.name).copied().unwrap_or(0);
            sum += n;
            out.push_str(&format!("    {}. {}（{} 个文件）\n", i + 1, d.name, n));
        }
        out.push_str(&format!(
            "  （以上子目录合计 {} 个资源文件）\n",
            sum
        ));
    } else {
        let n = p.content_files.len();
        out.push_str(&format!(
            "  本目录为单册图像／文档资料，共 {} 个资源文件，不分卷。\n",
            n
        ));
        out.push_str(&sample_names_line(&p.content_files));
    }
    out
}

fn sample_names_line(files: &[String]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let base = |f: &str| f.rsplit('/').next().unwrap_or(f).to_string();
    let n = files.len();
    let mut names: Vec<String> = Vec::new();
    for f in files.iter().take(3) {
        names.push(base(f));
    }
    if n > 6 {
        names.push("……".to_string());
        for f in files.iter().skip(n - 3) {
            names.push(base(f));
        }
    }
    format!("  文件名示例：{}\n", names.join("、"))
}

fn audio_format_line(unit_path: &Path, p: &Probe) -> String {
    if p.audios.is_empty() {
        return "本地资料未记录".into();
    }
    let infos = probe_audio(unit_path, &p.audios);
    infos
        .iter()
        .map(|i| match (i.rate, i.bits) {
            (Some(r), Some(b)) => format!("{}｜已检测采样率/位深：{}Hz / {}bit", i.format, r, b),
            _ => i.format.clone(),
        })
        .collect::<Vec<_>>()
        .join("｜")
}

/// 把说明文件写入单位目录（返回写入路径与渲染正文，避免调用方二次渲染）
pub fn write_note(input: &NoteInput) -> Result<(PathBuf, String), String> {
    let text = render_note(input);
    let path = write_note_file(input, &text)?;
    Ok((path, text))
}

/// 把**已渲染**的说明正文写入单位目录（UTF-8 BOM + CRLF），并清理旧的自动生成说明文件。
/// 导出流程用它避免二次渲染；跳过写说明时不会碰源目录（保住增量指纹）。
pub fn write_note_file(input: &NoteInput, text: &str) -> Result<PathBuf, String> {
    let dir = input.unit_path;
    // 清理旧的自动生成说明文件（避免同单位出现多份）
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path
                .extension()
                .map(|x| x.to_string_lossy().eq_ignore_ascii_case("txt"))
                == Some(true)
            {
            if let Ok(old) = fs::read(&path) {
                let head = String::from_utf8_lossy(&old[..old.len().min(4096)]);
                let is_target = path == dir.join(format!("{}.txt", input.entry.name));
                if head.contains(NOTE_FOOTER) && !is_target {
                    let _ = fs::remove_file(&path);
                }
            }
            }
        }
    }
    let target = dir.join(format!("{}.txt", input.entry.name));
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.replace('\n', "\r\n").replace("\r\r\n", "\r\n").as_bytes());
    fs::write(&target, bytes).map_err(|e| format!("写入 {} 失败: {}", target.display(), e))?;
    Ok(target)
}

/// 生成压缩注释用的说明文件正文（不带 BOM，保留 LF）
pub fn note_text(input: &NoteInput) -> String {
    render_note(input)
}
