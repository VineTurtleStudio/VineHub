use super::UnitStats;
use std::path::Path;

/// 说明文件 / 系统文件扩展名：不计入「内容文件」
const META_EXTS: &[&str] = &[
    "txt", "log", "bat", "bak", "cfg", "ini", "db", "url", "lnk", "nfo", "ds_store",
];

/// 纯系统垃圾文件（连「叶子判定」也不算）
const JUNK_NAMES: &[&str] = &["desktop.ini", "thumbs.db", ".ds_store", "folder.jpg"];

const CONTENT_EXTS: &[&str] = &[
    // 图像
    "jpg", "jpeg", "jfif", "png", "gif", "webp", "bmp", "tif", "tiff", "avif", "heic", "heif",
    "jxl",
    // 音频
    "flac", "mp3", "wav", "ape", "wv", "m4a", "m4b", "ogg", "oga", "aac", "alac", "opus",
    "wmv", "ac3", "dff", "dsf", "wma", "dts", "tta", "mpc", "ofr", "shn", "mka", "aiff",
    "aif", "ra", "mp2", "vqf",
    // 视频
    "mkv", "mp4", "avi", "mov", "flv", "ts", "m2ts", "mpg", "mpeg", "rm", "rmvb", "webm",
    "3gp", "vob",
    // 电子书 / 文档
    "epub", "pdf", "mobi", "azw", "azw3", "cbz", "cbr", "djvu", "doc", "docx", "rtf", "odt",
    "chm", "xlsx", "pptx",
    // 压缩包 / 镜像
    "rar", "7z", "zip", "tar", "gz", "xz", "iso", "bin", "img",
    // 字幕 / 歌词
    "lrc", "ass", "srt", "ssa", "vtt", "idx",
    // 可执行文件
    "exe",
];

pub fn extension_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

fn ext_lower(name: &str) -> String {
    extension_of(name).to_ascii_lowercase()
}

/// 是否为「内容文件」：构成压缩单位实体的资源文件
pub fn is_content_file(name: &str) -> bool {
    if is_junk_file(name) {
        return false;
    }
    let ext = ext_lower(name);
    if META_EXTS.contains(&ext.as_str()) {
        return false;
    }
    CONTENT_EXTS.contains(&ext.as_str())
}

/// 是否为纯系统垃圾（不算任何有效文件）
pub fn is_junk_file(name: &str) -> bool {
    JUNK_NAMES
        .iter()
        .any(|j| name.eq_ignore_ascii_case(j))
}

/// 叶子判定用：非系统垃圾的任意文件（.txt 也算）
pub fn is_meaningful_file(name: &str) -> bool {
    !is_junk_file(name)
}

/// 目录名是否为「碟片 / 分卷 / 内容切片」名（母文件夹判定用）
pub fn is_part_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return false;
    }
    let lower = n.to_lowercase();

    // 1) 常见切片标签（整词）
    const EXACT: &[&str] = &[
        "bk", "bd", "cd", "disc", "disk", "scan", "scans", "booklet", "booklets", "box",
        "flac", "lyrics", "inner", "outer", "cover", "covers", "card", "detail", "icon",
        "bonus", "other", "vip", "cdda", "cd-a", "blu-ray", "bluray", "dvd", "vcd", "bdrom",
        "cs edition", "music video dvd", "backgrounds", "dialog assets", "events",
        "gacha banners", "story", "units", "[overview]", "本体", "差分", "预览", "封面",
        "封底", "乐谱", "official score", "piano score",
    ];
    if EXACT.contains(&lower.as_str()) {
        return true;
    }
    const EXACT_CJK: &[&str] = &[
        "画像集", "ジャケット", "内页", "封皮", "扫描", "掃描", "内封", "背景", "歌词",
        "特典", "封面", "封底", "官方乐谱", "钢琴谱", "其他乐器",
    ];
    if EXACT_CJK.iter().any(|w| n == *w) {
        return true;
    }

    // 2) 第NN巻 / 第N章 / 第N話
    if starts_with_digit_marker(n) {
        return true;
    }

    // 3) Volume / Vol.
    if lower.starts_with("volume") || lower.starts_with("vol.") || lower.starts_with("vol ") {
        return true;
    }

    // 4) Type-A / Type B
    if lower.starts_with("type") {
        let rest = &lower[4..];
        if rest.starts_with('-') || rest.starts_with(' ') || rest.starts_with('_') {
            return true;
        }
    }

    // 5) ～盤 结尾（通常盤 / 限定盤 / 「ここさけ」盤 …）
    if n.ends_with('盤') || n.ends_with('盘') {
        return true;
    }

    // 6) Disc 1 / DISC1 / CD 2 / Disk-3
    if has_leading_disc_number(&lower) {
        return true;
    }

    // 7) Side A / Side B
    if lower.starts_with("side ") || lower.starts_with("side-") || lower == "side" {
        return true;
    }

    // 8) 短代号：HT01 / D1 / CD02 …（1~4 字母 + 1~3 数字）
    if is_short_code(n) {
        return true;
    }

    // 9) 方括号内 6~8 位纯数字（[220930] / [220602]）
    if has_bracketed_digits(n) {
        return true;
    }

    // 10) 任意位置 [Disk 1] / [Disc 2]
    if has_bracketed_disc(n) {
        return true;
    }

    false
}

fn starts_with_digit_marker(n: &str) -> bool {
    let bytes = n.as_bytes();
    if bytes.first() != Some(&0xe7) {
        // 「第」的 UTF-8 首字节 0xE7，快速排除
    }
    if !n.starts_with('第') {
        return false;
    }
    let rest = &n[3..]; // 「第」= 3 bytes
    let digits_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits_end == 0 {
        return false;
    }
    let tail = &rest[digits_end..];
    tail.starts_with(['巻', '卷', '冊', '册', '章', '話', '话', '集', '回'])
}

fn has_leading_disc_number(lower: &str) -> bool {
    for kw in ["disc", "disk", "cd"] {
        if let Some(rest) = lower.strip_prefix(kw) {
            let rest = rest.trim_start_matches(['-', '_', ' ']);
            if !rest.is_empty() && rest.as_bytes()[0].is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

fn is_short_code(n: &str) -> bool {
    let mut letters = 0usize;
    let mut digits = 0usize;
    for c in n.chars() {
        if c.is_ascii_alphabetic() {
            letters += 1;
        } else if c.is_ascii_digit() {
            digits += 1;
        } else {
            return false;
        }
    }
    letters >= 1 && letters <= 4 && digits >= 1 && digits <= 3 && letters + digits == n.len()
}

fn has_bracketed_digits(n: &str) -> bool {
    let bytes = n.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'[' {
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        let len = j - start;
        if (6..=8).contains(&len) && bytes.get(j) == Some(&b']') {
            return true;
        }
    }
    false
}

fn has_bracketed_disc(n: &str) -> bool {
    let lower = n.to_lowercase();
    for kw in ["[disk", "[disc", "[cd"] {
        if let Some(rest) = lower.strip_prefix(kw) {
            let rest = rest.trim_start_matches(['[', '-', '_', ' ']);
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    // 任意位置（例：「…Original Sound Track [Disk 1]」）
    if let Some(pos) = lower.find("[disk") {
        let rest = &lower[pos + 5..];
        let rest = rest.trim_start_matches([' ', '-']);
        if rest.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// 分类容器词：目录名命中即视为容器（继续下钻）
const CATEGORY_WORDS: &[&str] = &[
    "画集", "漫画", "图册", "小说", "同人誌", "日历", "乐谱", "卡片收藏", "壁纸", "电子书",
    "游戏", "回收", "资料", "设定集", "插画", "文档", "素材", "画廊", "单曲", "专辑",
    "音频", "视频", "漫画", "音乐",
];

pub fn is_category_container(name: &str) -> bool {
    if name.eq_ignore_ascii_case("CG") {
        return true;
    }
    CATEGORY_WORDS.iter().any(|w| name.contains(w))
}

/// 类别意图（路径/标题分类词）。按「最近段优先」匹配：单位自身名 > 直接祖先；
/// 同一段内多词命中时按 NOVEL → COMIC → ARTBOOK → MUSIC → LIVE → SCORE → VIDEO → DOC
/// 顺序取先者（历史兼容：`Key小说图册` 复合段判 novel）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cat {
    Novel,
    Comic,
    Artbook,
    Music,
    Live,
    Score,
    Video,
    Doc,
}

const NOVEL_WORDS: &[&str] = &["小说", "轻小说", "輕小說"];
const COMIC_WORDS: &[&str] = &["漫画", "漫畫"];
const ARTBOOK_WORDS: &[&str] = &[
    "画集", "图册", "同人誌", "卡片收藏", "壁纸", "设定集", "插画", "画师", "日历", "画廊", "游戏",
];
const MUSIC_WORDS: &[&str] = &["音乐", "音樂", "专辑", "專輯", "单曲", "單曲", "音频", "音訊"];
const LIVE_WORDS: &[&str] = &["演唱会", "演唱會", "公演", "ライブ"];
const SCORE_WORDS: &[&str] = &["乐谱", "樂譜", "楽譜"];
const VIDEO_WORDS: &[&str] = &["视频", "視頻", "番剧", "番組"];
const DOC_WORDS: &[&str] = &["资料", "資料", "文档", "文檔", "攻略", "ガイド"];

fn word_hit(seg: &str, image: bool) -> Option<Cat> {
    if NOVEL_WORDS.iter().any(|w| seg.contains(w)) {
        return Some(Cat::Novel);
    }
    if COMIC_WORDS.iter().any(|w| seg.contains(w)) || has_token(seg, "comic") {
        return Some(Cat::Comic);
    }
    if ARTBOOK_WORDS.iter().any(|w| seg.contains(w)) || has_token(seg, "cg") {
        return Some(Cat::Artbook);
    }
    if MUSIC_WORDS.iter().any(|w| seg.contains(w)) {
        return Some(Cat::Music);
    }
    if LIVE_WORDS.iter().any(|w| seg.contains(w)) {
        return Some(Cat::Live);
    }
    if SCORE_WORDS.iter().any(|w| seg.contains(w)) || has_token(seg, "score") {
        return Some(Cat::Score);
    }
    if VIDEO_WORDS.iter().any(|w| seg.contains(w)) {
        return Some(Cat::Video);
    }
    if DOC_WORDS.iter().any(|w| seg.contains(w)) {
        return Some(Cat::Doc);
    }
    // `guide` 是弱英文词：有图的「Official Guide Book」多为扫描画册，不作资料意图，继续向上找
    if !image && has_token(seg, "guide") {
        return Some(Cat::Doc);
    }
    None
}

/// 段名「恰为」类别词 = 分类文件夹意图明确，直接生效；
/// 类别词只是标题里的嵌入词（如 `XX画集特典CD`）= 模糊命中，交由内容信号裁决。
fn is_exact_segment(seg: &str, cat: Cat) -> bool {
    let s = seg.trim();
    let cjk: &[&str] = match cat {
        Cat::Novel => NOVEL_WORDS,
        Cat::Comic => COMIC_WORDS,
        Cat::Artbook => ARTBOOK_WORDS,
        Cat::Music => MUSIC_WORDS,
        Cat::Live => LIVE_WORDS,
        Cat::Score => SCORE_WORDS,
        Cat::Video => VIDEO_WORDS,
        Cat::Doc => DOC_WORDS,
    };
    if cjk.iter().any(|w| s == *w) {
        return true;
    }
    match cat {
        Cat::Artbook => s.eq_ignore_ascii_case("cg"),
        Cat::Comic => s.eq_ignore_ascii_case("comic"),
        Cat::Score => s.eq_ignore_ascii_case("score"),
        Cat::Doc => s.eq_ignore_ascii_case("guide"),
        _ => false,
    }
}

/// 路径任意位置存在音乐/公演语境（用于：音乐树里的视频 → 演唱会）。
fn has_music_ctx(rel_path: &str) -> bool {
    MUSIC_WORDS
        .iter()
        .chain(LIVE_WORDS.iter())
        .any(|w| rel_path.contains(w))
}

/// 大小写不敏感、两侧按 ASCII 字母断词的短词查找：
/// `CG`/`cg集`/`AIR CG` 命中，`OCG`/`MCG`/`MAGIC` 不命中。
fn has_token(seg: &str, token: &str) -> bool {
    let lower = seg.to_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while from + token.len() <= bytes.len() {
        match lower[from..].find(token) {
            None => return false,
            Some(pos) => {
                let i = from + pos;
                let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphabetic();
                let end = i + token.len();
                let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphabetic();
                if before_ok && after_ok {
                    return true;
                }
                from = i + 1;
            }
        }
    }
    false
}

/// hint 内容判定辅助：包含任意子串（文件名摘要已由 walk 拼接）。
fn hint_has(hint: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hint.contains(n))
}

/// 文件名 stem（去最后一个扩展名）
pub fn stem_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

fn is_pure_digits(s: &str, min: usize, max: usize) -> bool {
    s.len() >= min && s.len() <= max && s.chars().all(|c| c.is_ascii_digit())
}

/// 曲目编号命名：`01 - Title.flac` / `001.flac` / `2_Take.wav`（前导 1~3 位数字）
pub fn is_track_numbered(name: &str) -> bool {
    let stem = stem_of(name);
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 3 {
        return false;
    }
    if digits == stem.len() {
        return true;
    }
    matches!(
        stem[digits..].chars().next(),
        Some(' ' | '-' | '_' | '.' | '　')
    )
}

/// 连页编号命名：`001.jpg`（2~4 位纯数字）/ `p012.png` / `page_3.jpg`
pub fn is_page_numbered(name: &str) -> bool {
    let stem = stem_of(name);
    if is_pure_digits(stem, 2, 4) {
        return true;
    }
    let lower = stem.to_lowercase();
    for pre in ["page", "p"] {
        if let Some(rest) = lower.strip_prefix(pre) {
            let rest = rest.trim_start_matches(['_', '-', ' ']);
            if is_pure_digits(rest, 1, 4) {
                return true;
            }
        }
    }
    false
}

/// 集数命名：`S01E02.mkv` / `EP03.mp4` / `第01話.mkv` / `01.mkv`
pub fn is_episode_named(name: &str) -> bool {
    let stem = stem_of(name);
    if is_pure_digits(stem, 1, 3) {
        return true;
    }
    let lower = stem.to_lowercase();
    if let Some(rest) = lower.strip_prefix("ep") {
        let rest = rest.trim_start_matches(['_', '-', ' ', '.']);
        if is_pure_digits(rest, 1, 3) {
            return true;
        }
    }
    if let Some(rest) = stem.strip_prefix('第') {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && rest[digits..].starts_with(['話', '话', '集', '回', '章']) {
            return true;
        }
    }
    // SxxEyy：数字串前紧邻 s、后紧邻 e 且 e 后是数字
    let b = lower.as_bytes();
    for i in 1..b.len() {
        if b[i] == b'e' && b[i - 1].is_ascii_digit() {
            let mut j = i - 1;
            while j > 0 && b[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j > 0 && b[j - 1] == b's' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

/// 章回子目录名：`第01章` / `第3話` / `ch01` / `chapter 02`
pub fn is_chapter_dir(name: &str) -> bool {
    let n = name.trim();
    let lower = n.to_lowercase();
    if let Some(rest) = lower.strip_prefix("chapter") {
        return rest
            .trim_start_matches([' ', '-', '_', '.'])
            .starts_with(|c: char| c.is_ascii_digit());
    }
    if let Some(rest) = lower.strip_prefix("ch") {
        return rest.starts_with(|c: char| c.is_ascii_digit());
    }
    if let Some(rest) = n.strip_prefix('第') {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            return rest[digits..].starts_with(['章', '話', '话', '回']);
        }
    }
    false
}

/// 评分层低置信阈值：top1 与 top2 分差低于该值 → 需人工复核
const REVIEW_MARGIN: f32 = 0.7;

/// 类别判定结果：类型 + 置信度（评分层 = top1−top2 分差；路径门命中 = 1.0）+ 复核标记
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KindVerdict {
    pub kind: &'static str,
    pub confidence: f32,
    pub review: bool,
}

/// 字节/计数占比：字节分项可用时按字节（主体判类），否则退回计数（单测 stats 通常只填计数）
fn share(count: u32, bytes: u64, total_count: u32, total_bytes: u64) -> f32 {
    if total_bytes > 0 {
        bytes as f32 / total_bytes as f32
    } else if total_count > 0 {
        count as f32 / total_count as f32
    } else {
        0.0
    }
}

/// 音视频混合内容：音频与视频都有实体且量级相近（较少数 ≥ 较多数的 20%）→ 判类天然含糊
/// （「作品目录里既有专辑又有动画」「专辑 + bonus MV 体量相当」），一律标复核交人工确认。
/// 字节分项可用时按字节，否则按文件计数；单侧为 0 或另一侧极大（纯主体）不触发。
pub fn is_mixed_av(stats: &UnitStats) -> bool {
    let (a, v) = if stats.audio_b + stats.video_b > 0 {
        (stats.audio_b as f64, stats.video_b as f64)
    } else {
        (stats.audio_count as f64, stats.video_count as f64)
    };
    if a <= 0.0 || v <= 0.0 {
        return false;
    }
    let min = a.min(v);
    let max = a.max(v);
    min >= max * 0.2
}

/// 多信号评分：无类别词路径按内容证据打分，返回按分数降序的 (kind, score)。
/// 分项：音频主体（曲目结构/cue/日志/曲号/分卷/内嵌封面匹配）、公演语境、影像主体
/// （字幕/集数）、图像主体（艺术词/封面匹配）、漫画结构（漫画词/压缩包/章节/连页）、
/// 文本主体（电子书/散文）、谱面词、文档主体（资料词/扫描书让位）。
/// 并列时稳定排序保序 = 固定优先序 album → concert → score → comic → artbook →
/// novel → document → video → other。
pub fn kind_scores(stats: &UnitStats) -> Vec<(&'static str, f32)> {
    let hint = stats.hint.to_lowercase();
    let count_total = stats.audio_count
        + stats.image_count
        + stats.video_count
        + stats.doc_count
        + stats.text_count
        + stats.archive_count;
    let byte_total = stats.audio_b
        + stats.image_b
        + stats.video_b
        + stats.doc_b
        + stats.text_b
        + stats.archive_b;
    let sa = share(stats.audio_count, stats.audio_b, count_total, byte_total);
    let si = share(stats.image_count, stats.image_b, count_total, byte_total);
    let sv = share(stats.video_count, stats.video_b, count_total, byte_total);
    let sd = share(stats.doc_count, stats.doc_b, count_total, byte_total);
    let st = share(stats.text_count, stats.text_b, count_total, byte_total);

    // hint 证据（文件名/子目录名摘要，已 lowercased）
    let ebook = stats.ebook_count > 0 || hint_has(&hint, &["epub", "mobi", "azw", "kindle"]);
    let comic_arch = stats.comic_arch > 0 || hint_has(&hint, &["cbz", "cbr"]);
    let comic_word = hint_has(&hint, &["comic", "漫画", "漫畫", "コミック", "4コマ"]);
    let novel_word = hint_has(&hint, &["novel", "小说", "轻小说", "輕小說"]);
    let live_word =
        has_token(&hint, "live") || hint_has(&hint, &["公演", "演唱会", "演唱會", "ライブ"]);
    let music_word = has_token(&hint, "flac")
        || has_token(&hint, "ost")
        || has_token(&hint, "album")
        || hint_has(&hint, &["专辑", "專輯"]);
    let art_word = hint_has(&hint, &["artbook", "画集", "設定集", "设定集", "illustration"])
        || has_token(&hint, "cg");
    let score_word =
        has_token(&hint, "score") || hint_has(&hint, &["乐谱", "樂譜", "楽譜", "sheet music"]);
    let doc_word = hint_has(
        &hint,
        &["manual", "guide", "说明书", "说明書", "资料", "資料", "readme"],
    );

    // ---- album 专辑：音频主体 + 抓轨结构证据 ----
    let mut album = 0.0;
    if stats.audio_count > 0 {
        album += 2.0;
    }
    album += 3.0 * sa;
    if stats.cue_count > 0 {
        album += 1.0;
    }
    if stats.log_count > 0 {
        album += 0.8;
    }
    if stats.tracks_numbered >= 3 {
        album += 1.0;
    } else if stats.tracks_numbered > 0 {
        album += 0.4;
    }
    if music_word {
        album += 0.8;
    }
    if stats.part_dirs >= 2 {
        album += 0.6;
    }
    if stats.audio_count > 0 && stats.video_count > 0 {
        album += 0.5; // 发行物带 bonus MV
    }
    if let Some(r) = stats.cover_match {
        album += 3.0 * r; // 散图与内嵌封面一致 = 图片只是专辑封面
    }

    // ---- concert 演唱会：公演语境 + 影像/音频 ----
    let mut concert = 0.0;
    if live_word {
        concert += 2.5;
        if music_word {
            concert += 1.0;
        }
        concert += 1.5 * sv;
        if stats.audio_count > 0 {
            concert += 1.0;
        }
    }

    // ---- video 视频：影像主体 + 字幕/集数结构 ----
    let mut video = 0.0;
    if stats.video_count > 0 {
        video += 2.0;
    }
    video += 3.5 * sv;
    if stats.sub_count > 0 {
        video += 1.0;
    }
    if stats.episode_count >= 2 {
        video += 1.2;
    } else if stats.episode_count > 0 {
        video += 0.5;
    }

    // ---- artbook 图册：图像主体 ----
    let mut artbook = 0.0;
    if stats.image_count > 0 {
        artbook += 2.0;
    }
    artbook += 3.0 * si;
    if art_word {
        artbook += 1.5;
    }
    if let Some(r) = stats.cover_match {
        if stats.audio_count > 0 && stats.image_count > 0 {
            artbook += 1.5 * (1.0 - r); // 图不匹配内嵌封面 → 真扫描图而非封面
        }
    }

    // ---- comic 漫画：图册基底 + 漫画结构证据（无漫画证据不与图册争）----
    let mut comic = 0.0;
    if comic_word || comic_arch {
        comic = artbook + 3.0;
    } else if stats.chapter_dirs > 0 && stats.image_count > 0 {
        comic = artbook + 1.5;
    }
    if comic > 0.0 && stats.page_numbered >= 20 {
        comic += 0.8; // 长连页
    }

    // ---- novel 小说：文本主体/电子书 ----
    let mut novel = 0.0;
    if ebook {
        novel += 3.5;
    }
    if stats.text_count > 0 && stats.image_count <= 3 && stats.audio_count == 0 {
        novel += 1.5;
        novel += 1.5 * st;
    }
    if stats.prose_count >= 3 && stats.doc_count == 0 && stats.audio_count == 0 {
        novel += 1.0;
    }
    if novel_word {
        novel += 1.2;
    }

    // ---- score 乐谱：谱面词 + 无音频 ----
    let mut score = 0.0;
    if score_word && stats.audio_count == 0 {
        score = 3.0;
        if stats.image_count > 0 {
            score += 0.5;
        }
        if stats.doc_count > 0 {
            score += 0.5;
        }
    }

    // ---- document 资料：文档主体（电子书/漫画包让位，扫描书让位图册）----
    let mut document = 0.0;
    if stats.doc_count > 0 {
        document += 2.0;
    }
    document += 3.0 * sd;
    if doc_word {
        document += 1.0;
    }
    if ebook {
        document *= 0.3;
    }
    if comic_arch {
        document *= 0.3;
    }
    if stats.image_count > 0 && si > 0.6 {
        document *= 0.5;
    }

    let other = 0.4;

    let mut v = vec![
        ("album", album),
        ("concert", concert),
        ("score", score),
        ("comic", comic),
        ("artbook", artbook),
        ("novel", novel),
        ("document", document),
        ("video", video),
        ("other", other),
    ];
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// 判定单位类型并给出置信度/复核标记（9 类）：
/// album 专辑 / novel 小说 / artbook 图册 / comic 漫画 / score 乐谱 /
/// concert 演唱会 / video 视频 / document 资料 / other 其它。
///
/// 1. 「最近段」类别词决定路径意图（自身名 > 直接祖先）；
/// 2. 段名恰为类别词（分类文件夹）→ 意图直接生效；类别词仅嵌在标题里 → 内容裁决，
///    **含音频一律专辑**（修复「专辑/画集特典 CD 被识别成图册或小说」）；
///    音乐/公演语境 + 视频 → 演唱会；乐谱 → 乐谱；`guide` 弱词有图时让位向上。
///    路径门命中 = 高置信（confidence 1.0）；但音视频体量相近（`is_mixed_av`）时仍标复核。
/// 3. 无类别词 → `kind_scores` 多信号评分取最高分（主体判类：字节占比优先，
///    无字节时退回计数）；top1−top2 < `REVIEW_MARGIN`、判为 other 或音视频混合 →
///    `review = true` 交人工复核。
pub fn kind_verdict(rel_path: &str, stats: &UnitStats) -> KindVerdict {
    let audio = stats.audio_count > 0;
    let image = stats.image_count > 0;
    let video = stats.video_count > 0;
    let hint = stats.hint.to_lowercase();
    let music_ctx = has_music_ctx(rel_path);

    // 1) 最近段类别意图（idx = 命中段距单位名的深度：0 = 单位自身名）
    let mut primary: Option<(Cat, bool, usize)> = None;
    for (idx, seg) in rel_path.rsplit('/').enumerate() {
        if let Some(cat) = word_hit(seg, image) {
            primary = Some((cat, is_exact_segment(seg, cat), idx));
            break;
        }
    }

    // 2) 意图映射（路径门）
    if let Some((cat, exact, idx)) = primary {
        use Cat::*;
        let kind = match cat {
            Novel => {
                if exact {
                    "novel"
                } else if audio {
                    "album"
                } else if hint_has(&hint, &["epub", "mobi", "azw", "kindle"]) {
                    "novel"
                } else if idx > 0 && video {
                    // 远距根词（如 `Key小说图册`）不抢图像/视频内容：CG、卡片、扫描归内容裁决
                    "video"
                } else if idx > 0 && image {
                    "artbook"
                } else {
                    "novel"
                }
            }
            Comic => {
                if exact || !audio {
                    "comic"
                } else {
                    "album"
                }
            }
            Artbook => {
                if exact || !audio {
                    "artbook"
                } else {
                    "album"
                }
            }
            Music => {
                // 发行物带 bonus MV（音频+视频共存）仍是专辑；纯视频内容 = 演唱会/公演影像
                if video && !audio {
                    "concert"
                } else {
                    "album"
                }
            }
            Live => "concert",
            Score => "score",
            Video => {
                if music_ctx && video && !audio {
                    "concert"
                } else if exact || !audio {
                    "video"
                } else {
                    "album"
                }
            }
            Doc => {
                if exact || !audio {
                    "document"
                } else {
                    "album"
                }
            }
        };
        return KindVerdict {
            kind,
            confidence: 1.0,
            // 路径门高置信，但音视频体量相近的混合内容仍提示人工确认（kind 不变）
            review: is_mixed_av(stats),
        };
    }

    // 3) 无类别词：多信号评分
    let scores = kind_scores(stats);
    let (kind, top) = scores[0];
    let confidence = top - scores[1].1;
    let review = confidence < REVIEW_MARGIN || kind == "other" || is_mixed_av(stats);
    KindVerdict {
        kind,
        confidence,
        review,
    }
}

/// 根据路径、内容与文件名摘要推断条目类型（9 类），判定语义见 `kind_verdict`
pub fn infer_kind(rel_path: &str, stats: &UnitStats) -> String {
    kind_verdict(rel_path, stats).kind.to_string()
}

pub fn join_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", parent, name)
    }
}

pub fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_and_meta() {
        assert!(is_content_file("a.jpg"));
        assert!(is_content_file("b.FLAC"));
        assert!(!is_content_file("KSLA.txt"));
        assert!(!is_content_file("desktop.ini"));
        assert!(!is_content_file("Thumbs.db"));
        assert!(!is_content_file("01_Key Sounds Label.txt"));
        assert!(!is_content_file("x.log"));
        assert!(is_content_file("a.wma"));
        assert!(is_content_file("b.dts"));
        assert!(is_meaningful_file("_回收待删.txt"));
        assert!(!is_meaningful_file("desktop.ini"));
    }

    fn st(audio: u32, image: u32, text: u32, video: u32) -> UnitStats {
        UnitStats {
            audio_count: audio,
            image_count: image,
            text_count: text,
            video_count: video,
            ..Default::default()
        }
    }

    fn st_hint(doc: u32, hint: &str) -> UnitStats {
        UnitStats {
            doc_count: doc,
            hint: hint.into(),
            ..Default::default()
        }
    }

    #[test]
    fn kind_inference() {
        // 分类文件夹意图直接生效（画集 + 附赠 CD 仍为图册）
        assert_eq!(infer_kind("X/画集/某某特典CD", &st(2, 30, 0, 0)), "artbook");
        // 类别词仅嵌在标题里 + 含音频 → 专辑（修复 album→artbook/novel）
        assert_eq!(infer_kind("X/发卖画集特典CD", &st(4, 6, 0, 0)), "album");
        assert_eq!(infer_kind("XX小说限定版CD", &st(6, 2, 0, 0)), "album");
        // OCG/MCG 不命中 CG 断词
        assert_eq!(infer_kind("专辑/OCG精选", &st(8, 2, 0, 0)), "album");
        assert_eq!(infer_kind("Plain/MCG-100", &st(6, 1, 0, 0)), "album");
        // 最近段优先：Key小说图册 根下的画集/漫画/CG/小说/乐谱子树各归其类
        assert_eq!(infer_kind("Key小说图册/A/画集/Kanon Fanbook", &st(0, 80, 0, 0)), "artbook");
        assert_eq!(infer_kind("Key小说图册/A/漫画/某漫画", &st(0, 40, 0, 0)), "comic");
        assert_eq!(infer_kind("Key小说图册/A/CG/Kanon CG", &st(0, 10, 0, 0)), "artbook");
        assert_eq!(infer_kind("Key小说图册/A/小说/某小说", &st(0, 3, 12, 0)), "novel");
        assert_eq!(infer_kind("Key小说图册/A/乐谱/钢琴谱", &st(0, 5, 2, 0)), "score");
        // 远距根词（Key小说图册）不抢图像/视频内容：CG、游戏、卡片 → 内容/就近分类裁决
        assert_eq!(infer_kind("Key小说图册/Key 官方作品/028 某作品", &st(0, 1277, 2, 0)), "artbook");
        assert_eq!(infer_kind("Key小说图册/Key 官方作品/游戏/Rewrite+", &st(0, 491, 1, 0)), "artbook");
        assert_eq!(infer_kind("Key小说图册/跨作品/某Characters", &st(0, 12, 1, 0)), "artbook");
        assert_eq!(infer_kind("Key小说图册/跨作品/某动画", &st(0, 1, 1, 6)), "video");
        // 分类文件夹意图含音频仍按意图（小说夹里的广播剧 = 小说；漫画夹里的 CD = 漫画）
        assert_eq!(infer_kind("小说/某广播剧", &st(6, 1, 3, 0)), "novel");
        assert_eq!(infer_kind("漫画/某CD", &st(4, 10, 1, 0)), "comic");
        // 音乐/公演语境 + 视频 → 演唱会
        assert_eq!(infer_kind("KEY音乐/某LIVE[video-mp4]", &st(0, 1, 1, 1)), "concert");
        assert_eq!(infer_kind("音乐/某特典", &st(0, 1, 0, 6)), "concert");
        assert_eq!(infer_kind("音乐/纯视频特典", &st(0, 1, 0, 6)), "concert");
        assert_eq!(infer_kind("KEY音乐/公演/[2012] Live Blu-ray", &st(0, 2, 1, 3)), "concert");
        // 但发行物带 bonus MV（音频+视频共存）仍是专辑；公演词命中 = 演唱会（音频现场亦可）
        assert_eq!(infer_kind("KEY音乐/Bonus MV 单曲", &st(6, 4, 1, 3)), "album");
        assert_eq!(infer_kind("X/[2010] ライブパンフレット", &st(1, 3, 1, 0)), "concert");
        // 日文楽譜词命中
        assert_eq!(infer_kind("X/Angel Beats! 楽譜集", &st(0, 9, 1, 0)), "score");
        // `guide` 弱词：有图时让位向上（扫描画册 = 图册），无图时判资料
        assert_eq!(infer_kind("X/画集/Official Guide Book", &st(0, 249, 1, 0)), "artbook");
        assert_eq!(infer_kind("X/官方攻略本", &st(0, 0, 2, 0)), "document");
        assert_eq!(infer_kind("资料/使用说明", &st(0, 0, 3, 0)), "document");
        // 无类别词走内容构成
        assert_eq!(infer_kind("Plain/albumX", &st(12, 4, 0, 0)), "album");
        assert_eq!(infer_kind("Plain/videoX", &st(0, 1, 0, 6)), "video");
        assert_eq!(infer_kind("Plain/novelX", &st(0, 0, 5, 0)), "novel");
        assert_eq!(infer_kind("Plain/bookX", &st(0, 9, 0, 0)), "artbook");
        assert_eq!(infer_kind("Plain/emptyX", &st(0, 0, 0, 0)), "other");
        // 长名单元：纯视频 → 视频，非音乐语境不吞演唱会；有音轨 → 专辑
        assert_eq!(infer_kind("X/Some.BDRemux.FLAC配信", &st(1, 1, 1, 1)), "album");
        assert_eq!(infer_kind("X/Some.BDRemux", &st(0, 1, 1, 2)), "video");
        // hint 文件名摘要：电子书 → 小说；漫画压缩包/漫画词 → 漫画；PDF 纯文档 → 资料
        assert_eq!(infer_kind("Plain/ebook", &st_hint(1, " novel.epub ")), "novel");
        assert_eq!(infer_kind("Plain/pack", &st_hint(1, " 01.cbz ")), "comic");
        let mut anth = st(0, 40, 1, 0);
        anth.hint = " xxx コミック 4コマ yyy ".into();
        assert_eq!(infer_kind("Plain/anthology", &anth), "comic");
        assert_eq!(infer_kind("Plain/handbook", &st_hint(2, " manual.pdf ")), "document");
    }

    /// 模式 helper：曲号 / 连页 / 集数 / 章回目录
    #[test]
    fn naming_patterns() {
        let track_yes = ["01 - Title.flac", "001.flac", "2_Take.wav", "09.m4a"];
        for n in track_yes {
            assert!(is_track_numbered(n), "expected track: {}", n);
        }
        let track_no = ["2019 - Live.flac", "10cm.flac", "intro.flac", "24bit.wav"];
        for n in track_no {
            assert!(!is_track_numbered(n), "expected NOT track: {}", n);
        }

        let page_yes = ["001.jpg", "12.png", "p012.png", "page_3.jpg", "P-007.jpeg"];
        for n in page_yes {
            assert!(is_page_numbered(n), "expected page: {}", n);
        }
        let page_no = ["1.jpg", "cover.png", "photo.jpg", "12345.jpg"];
        for n in page_no {
            assert!(!is_page_numbered(n), "expected NOT page: {}", n);
        }

        let ep_yes = ["01.mkv", "S01E02.mkv", "EP03.mp4", "ep03.mp4", "第01話.mkv", "第3集.mp4"];
        for n in ep_yes {
            assert!(is_episode_named(n), "expected episode: {}", n);
        }
        let ep_no = ["2019.mkv", "movie.mkv", "cover.jpg", "EPIC03.mkv"];
        for n in ep_no {
            assert!(!is_episode_named(n), "expected NOT episode: {}", n);
        }

        let ch_yes = ["第01章", "第3話", "ch01", "chapter 02", "CH12"];
        for n in ch_yes {
            assert!(is_chapter_dir(n), "expected chapter: {}", n);
        }
        let ch_no = ["第01巻", "chapter", "cover", "Chaos"];
        for n in ch_no {
            assert!(!is_chapter_dir(n), "expected NOT chapter: {}", n);
        }
    }

    /// 评分层：证据字段驱动的散堆判类 + 置信/复核
    #[test]
    fn kind_scoring() {
        // 曲目结构 + cue/日志 → 专辑（无路径词散堆）
        let mut rip = st(10, 1, 1, 0);
        rip.cue_count = 1;
        rip.log_count = 1;
        rip.tracks_numbered = 10;
        assert_eq!(infer_kind("Plain/rip", &rip), "album");

        // 视频主体 + 字幕 + 集数 → 视频
        let mut series = st(0, 1, 0, 6);
        series.sub_count = 6;
        series.episode_count = 6;
        assert_eq!(infer_kind("Plain/series", &series), "video");

        // 图像主体 + 章回目录 + 连页 → 漫画
        let mut pages = st(0, 40, 1, 0);
        pages.chapter_dirs = 5;
        pages.page_numbered = 38;
        assert_eq!(infer_kind("Plain/pages", &pages), "comic");

        // 字节主体判类：1 首音频但图字节占优 → 图册；音频字节占优 → 专辑
        let mut audio_dom = st(4, 2, 0, 0);
        audio_dom.audio_b = 40_000_000;
        audio_dom.image_b = 2_000_000;
        assert_eq!(infer_kind("Plain/byteA", &audio_dom), "album");
        let mut image_dom = st(4, 2, 0, 0);
        image_dom.audio_b = 1_000_000;
        image_dom.image_b = 40_000_000;
        assert_eq!(infer_kind("Plain/byteI", &image_dom), "artbook");

        // 封面匹配：散图 = 内嵌封面 → 专辑；不匹配 + 图占优 → 图册
        let mut matched = st(1, 3, 0, 0);
        matched.cover_match = Some(1.0);
        assert_eq!(infer_kind("Plain/cov1", &matched), "album");
        let mut diff = st(1, 40, 0, 0);
        diff.cover_match = Some(0.0);
        assert_eq!(infer_kind("Plain/cov0", &diff), "artbook");

        // 空单位 → other 且进复核
        let v = kind_verdict("Plain/empty", &UnitStats::default());
        assert_eq!(v.kind, "other");
        assert!(v.review, "空单位应标人工复核");

        // 路径门命中 = 高置信不复核；评分层高分差也不复核；分差小 → 复核
        let gate = kind_verdict("X/画集/某画册", &st(0, 30, 0, 0));
        assert!(!gate.review && gate.confidence >= 1.0);
        let clear = kind_verdict("Plain/albumX", &st(12, 4, 0, 0));
        assert!(!clear.review && clear.confidence >= 0.7);
        let tie = kind_verdict("X/Some.BDRemux.FLAC配信", &st(1, 1, 1, 1));
        assert!(tie.review, "音频/图/视频均分应低置信复核: {:?}", tie);
    }

    /// 音视频混合内容（作品目录既有专辑又有动画 / 体量相当的 bonus MV）→ 一律复核
    #[test]
    fn mixed_av_review() {
        // 计数回落路径：12 音频 + 4 视频（4/12 ≥ 0.2）→ 混合
        let mix = st(12, 0, 0, 4);
        assert!(is_mixed_av(&mix));
        let v = kind_verdict("Plain/Work", &mix);
        assert!(v.review, "音视频混合应复核: {:?}", v);

        // 字节路径：300MB 音频 + 600MB 视频 → 混合；400MB + 8GB → 单侧压倒不触发
        let mut near = st(12, 0, 0, 4);
        near.audio_b = 300_000_000;
        near.video_b = 600_000_000;
        assert!(is_mixed_av(&near));
        let mut skewed = st(12, 2, 0, 1);
        skewed.audio_b = 400_000_000;
        skewed.video_b = 8_000_000_000;
        assert!(!is_mixed_av(&skewed));

        // 路径门命中但内容混合 → 照样复核（kind 仍是门命中结果，confidence 不变）
        let gated = kind_verdict("专辑A", &mix);
        assert_eq!(gated.kind, "album");
        assert!(gated.review && gated.confidence >= 1.0);

        // 纯音频 / 纯视频 / 无实体不触发
        assert!(!is_mixed_av(&st(12, 0, 0, 0)));
        assert!(!is_mixed_av(&st(0, 0, 0, 6)));
        assert!(!is_mixed_av(&UnitStats::default()));
    }

    #[test]
    fn part_names() {
        let yes = [
            "Type-A", "通常盤", "セブン-イレブン限定盤", "BK", "Scan", "Scans", "booklet",
            "第01巻", "第0章", "Volume 01", "Volume 1", "Vol.00", "Vol.05 EXTRA01",
            "HT01", "HT04", "Disc 1", "DISC2", "CD 3", "Side A", "FLAC", "画像集",
            "ジャケット", "内页", "封皮", "Card", "Detail", "Icon", "[Overview]",
            "Music Video DVD", "CS Edition", "backgrounds", "dialog assets", "events",
            "gacha banners", "story", "units", "官方乐谱", "钢琴谱", "其他乐器",
            "Disc1(Original Side)", "Side A - Original Side", "BOX",
            "[220930]終のステラ オリジナルサウンドトラック[320K]",
            "[Hi-Res][220602]Key『Heaven Burns Red』主题歌「Before I Rise」[FLAC]",
            "2017 - Rewrite Animation Original Sound Track [Disk 1]",
        ];
        for n in yes {
            assert!(is_part_name(n), "expected part: {}", n);
        }

        let no = [
            "AIR CG", "KSLA", "Black Rabbit", "Kanon Fanbook", "HAPPY TURN EX",
            "[2001.08.10][KSLA-0001] WORK-S Humanity／WORK-S [cd-wav+cue+log+img]",
            "1999 Kanon（カノン）", "通常集", "Typeface", "Discount Store", "CDCollection",
        ];
        for n in no {
            assert!(!is_part_name(n), "expected NOT part: {}", n);
        }
    }

    #[test]
    fn categories() {
        assert!(is_category_container("画集"));
        assert!(is_category_container("_回收待删"));
        assert!(is_category_container("漫画"));
        assert!(is_category_container("CG"));
        assert!(!is_category_container("KSLA"));
        assert!(!is_category_container("MAGIC"));
    }
}
