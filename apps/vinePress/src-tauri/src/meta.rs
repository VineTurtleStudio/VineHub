//! 自动识别填充：从单位目录内的实际文件推断条目元数据。
//! 只产出「提案」（新值 + 来源 + 原值），是否应用由前端逐条确认。

use crate::project::{split_work_and_category, Entry};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 可自动填充的字段（id, 展示名）
pub const FIELDS: &[(&str, &str)] = &[
    ("title", "标题"),
    ("artist", "艺人"),
    ("date", "发行日期"),
    ("label", "唱片公司"),
    ("catalog", "规格品番"),
    ("publisher", "出版方"),
    ("rank", "评级"),
    ("tracklist", "曲目列表"),
    ("tags", "标签"),
    ("notes", "备注"),
    ("work", "所属作品"),
    ("category", "分类"),
];

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub rel_path: String,
    pub field: String,
    pub value: String,
    pub source: String,
    pub old: String,
}

/// 字段 -> (值, 来源)
type Found = HashMap<String, (String, String)>;

// ---------------------------------------------------------------- 入口

pub fn propose_all(root: &Path, entries: &[Entry], fields: &[String]) -> Vec<Proposal> {
    let mut out = Vec::new();
    for e in entries {
        let unit = join(root, &e.rel_path);
        if !unit.is_dir() {
            continue;
        }
        for (field, (value, source)) in extract(e, &unit, fields) {
            let old = current_value(e, &field);
            if value.is_empty() || normalize(&value) == normalize(&old) {
                continue;
            }
            out.push(Proposal {
                rel_path: e.rel_path.clone(),
                field,
                value,
                source,
                old,
            });
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path).then_with(|| a.field.cmp(&b.field)));
    out
}

pub fn extract(entry: &Entry, unit: &Path, fields: &[String]) -> Found {
    let want = |f: &str| fields.iter().any(|x| x == f);
    let unit_name = entry.name.clone();

    // 单次目录遍历，供说明文件 / readme / CUE / 音频 / 构成标签共用
    let idx = FileIndex::new(unit);

    let note = find_note(&idx, &unit_name);
    let note_map = note.as_deref().map(parse_note).unwrap_or_default();

    let dir = parse_dir_name(&unit_name);
    let mut found: Found = HashMap::new();

    // 音频标签（惰性：仅在需要且说明文件缺失时读取文件头）
    let need_tag_field = want("title") || want("artist") || want("date") || want("label") || want("catalog");
    let audio_cache: RefCell<Option<AudioTags>> = RefCell::new(None);
    let audio = || -> AudioTags {
        let mut slot = audio_cache.borrow_mut();
        if slot.is_none() {
            *slot = Some(read_tags_from(&idx));
        }
        slot.clone().unwrap_or_default()
    };

    if want("title") {
        if let Some(v) = note_value(&note_map, &["专辑名称", "作品名称", "标题"]) {
            found.insert("title".into(), (v, "已有说明文件".into()));
        } else {
            let from_tag = need_tag_field.then(|| audio()).and_then(|t| nonempty(if !t.album.is_empty() { t.album } else { t.title }));
            if let Some(v) = from_tag {
                found.insert("title".into(), (v, "音频标签".into()));
            } else if !dir.title.is_empty() {
                found.insert("title".into(), (dir.title.clone(), "目录名".into()));
            }
        }
    }
    if want("artist") {
        if let Some(v) = note_value(&note_map, &["艺人", "画师", "作者"]) {
            found.insert("artist".into(), (v, "已有说明文件".into()));
        } else {
            let from_tag = need_tag_field.then(|| audio()).and_then(|t| nonempty(t.artist));
            if let Some(v) = from_tag {
                found.insert("artist".into(), (v, "音频标签".into()));
            } else if !dir.artist.is_empty() {
                found.insert("artist".into(), (dir.artist.clone(), "目录名".into()));
            }
        }
    }
    if want("date") {
        if let Some(v) = note_value(&note_map, &["发行日期", "日期"]) {
            found.insert("date".into(), (v, "已有说明文件".into()));
        } else {
            let from_tag = need_tag_field.then(|| audio()).and_then(|t| nonempty(t.date));
            if let Some(v) = from_tag {
                found.insert("date".into(), (v, "音频标签".into()));
            } else if !dir.date.is_empty() {
                found.insert("date".into(), (dir.date.clone(), "目录名".into()));
            }
        }
    }
    if want("catalog") {
        if let Some(v) = note_value(&note_map, &["规格品番", "品番"]) {
            found.insert("catalog".into(), (v, "已有说明文件".into()));
        } else {
            let from_tag = need_tag_field.then(|| audio()).and_then(|t| nonempty(t.catalog));
            if let Some(v) = from_tag {
                found.insert("catalog".into(), (v, "音频标签".into()));
            } else if !dir.catalog.is_empty() {
                found.insert("catalog".into(), (dir.catalog.clone(), "目录名".into()));
            }
        }
    }
    if want("label") {
        if let Some(v) = note_value(&note_map, &["唱片公司", "厂牌", "唱片品牌"]) {
            found.insert("label".into(), (v, "已有说明文件".into()));
        } else {
            let from_tag = need_tag_field.then(|| audio()).and_then(|t| nonempty(t.label));
            if let Some(v) = from_tag {
                found.insert("label".into(), (v, "音频标签".into()));
            } else if let Some(prefix) = dir.catalog.split('-').next() {
                if prefix.len() >= 2 && prefix.chars().all(|c| c.is_ascii_alphabetic()) {
                    found.insert("label".into(), (prefix.to_string(), "目录号前缀（推测）".into()));
                }
            }
        }
    }
    if want("publisher") {
        if let Some(v) = note_value(&note_map, &["出版方／发行商", "出版方", "出版社", "发行商"]) {
            found.insert("publisher".into(), (v, "已有说明文件".into()));
        }
    }
    if want("rank") {
        if let Some(v) = note_value(&note_map, &["最高位", "评级"]) {
            found.insert("rank".into(), (v, "已有说明文件".into()));
        }
    }
    if want("work") && entry.work.is_empty() {
        let (work, _) = split_work_and_category(&entry.rel_path);
        if !work.is_empty() {
            found.insert("work".into(), (work, "路径结构".into()));
        }
    }
    if want("category") && entry.category.is_empty() {
        let (_, category) = split_work_and_category(&entry.rel_path);
        if !category.is_empty() {
            found.insert("category".into(), (category, "路径结构".into()));
        }
    }

    if want("tracklist") {
        let from_note = note.as_deref().map(note_tracklist_from_text).unwrap_or_default();
        let tracks = if !from_note.is_empty() {
            from_note
        } else if let Some(t) = cue_tracklist(&idx) {
            t
        } else {
            numbered_audio_names(&idx)
        };
        if !tracks.is_empty() {
            found.insert("tracklist".into(), (tracks.join("\n"), "说明文件／CUE／文件名".into()));
        }
    }

    if want("tags") {
        let tags = composition_tags(&idx, entry);
        if !tags.is_empty() {
            found.insert("tags".into(), (tags.join("、"), "文件构成".into()));
        }
    }

    if want("notes") {
        if let Some(v) = note_value(&note_map, &["备注", "注释"]) {
            found.insert("notes".into(), (v, "已有说明文件".into()));
        } else if let Some(t) = find_readme(&idx) {
            found.insert("notes".into(), (t, "目录内说明文本".into()));
        }
    }

    found
}

/// 单位内文件清单：一次遍历，多处复用（上限 1200 个文件）
pub struct FileIndex {
    unit: PathBuf,
    files: Vec<PathBuf>,
}

impl FileIndex {
    pub fn new(unit: &Path) -> FileIndex {
        FileIndex {
            unit: unit.to_path_buf(),
            files: walk_limited(unit, 1200),
        }
    }

    fn top_level(&self) -> Vec<&PathBuf> {
        self.files
            .iter()
            .filter(|p| p.parent() == Some(self.unit.as_path()))
            .collect()
    }
}

fn join(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

fn current_value(e: &Entry, field: &str) -> String {
    match field {
        "title" => e.title.clone(),
        "artist" => e.artist.clone(),
        "date" => e.date.clone(),
        "label" => e.label.clone(),
        "catalog" => e.catalog.clone(),
        "publisher" => e.publisher.clone(),
        "rank" => e.rank.clone(),
        "tracklist" => e.tracklist.join("\n"),
        "tags" => e.tags.join("、"),
        "notes" => e.notes.clone(),
        "work" => e.work.clone(),
        "category" => e.category.clone(),
        _ => String::new(),
    }
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_placeholder(v: &str) -> bool {
    let v = v.trim();
    v.is_empty()
        || v.contains("本地资料未记录")
        || v.contains("未检测到")
        || v.contains("（未")
        || v.contains("(未")
}

// ---------------------------------------------------------------- 目录名解析

pub struct DirInfo {
    pub title: String,
    pub artist: String,
    pub date: String,
    pub catalog: String,
}

pub fn parse_dir_name(name: &str) -> DirInfo {
    let mut artist = String::new();
    let mut base = name.to_string();

    // `...／艺人`（全角斜杠）或 `... / 艺人`
    if let Some(i) = base.find('／') {
        let right = base[i + '／'.len_utf8()..].trim().to_string();
        let right = clean_title(&right);
        if !right.is_empty() && right.chars().count() <= 40 {
            artist = right;
            base.truncate(i);
        }
    } else if let Some(i) = base.find(" / ") {
        let right = base[i + 3..].trim().to_string();
        let right = clean_title(&right);
        if !right.is_empty() && right.chars().count() <= 40 {
            artist = right;
            base.truncate(i);
        }
    }

    let date = find_date(&base);
    let catalog = find_catalog(&base);
    let mut title = clean_title(&base);
    if title.is_empty() {
        title = name.to_string();
    }
    DirInfo {
        title,
        artist,
        date,
        catalog,
    }
}

/// 目录名里的日期：`2001.08.10` / `2001-08-10` / `(2001)` / `（2001）`
pub fn find_date(s: &str) -> String {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i + 10 <= b.len() {
        let numeric = |k: usize| b[i + k].is_ascii_digit();
        if numeric(0) && numeric(1) && numeric(2) && numeric(3)
            && matches!(b[i + 4], b'.' | b'-' | b'/')
            && numeric(5)
            && numeric(6)
            && matches!(b[i + 7], b'.' | b'-' | b'/')
            && numeric(8)
            && numeric(9)
        {
            return format!("{}-{}-{}", &s[i..i + 4], &s[i + 5..i + 7], &s[i + 8..i + 10]);
        }
        i += 1;
    }
    for (open, close) in [('(', ')'), ('（', '）'), ('[', ']')] {
        let mut start = 0usize;
        while let Some(p) = s[start..].find(open) {
            let ps = start + p;
            if let Some(e) = s[ps + open.len_utf8()..].find(close) {
                let inner = s[ps + open.len_utf8()..ps + open.len_utf8() + e].trim();
                if inner.len() == 4
                    && inner.bytes().all(|c| c.is_ascii_digit())
                    && matches!(inner.as_bytes()[0], b'1' | b'2')
                {
                    return inner.to_string();
                }
            }
            start = ps + open.len_utf8();
        }
    }
    String::new()
}

/// 目录名里的品番：`[KSLA-0098]`、`[SRCL-9974]`、`[VTCL-35098]`
pub fn find_catalog(s: &str) -> String {
    let mut start = 0usize;
    while let Some(p) = s[start..].find('[') {
        let ps = start + p;
        if let Some(e) = s[ps + 1..].find(']') {
            let inner = s[ps + 1..ps + 1 + e].trim().to_string();
            if is_catalog_like(&inner) {
                return inner;
            }
            start = ps + 1 + e + 1;
        } else {
            break;
        }
    }
    String::new()
}

fn is_catalog_like(s: &str) -> bool {
    let mut has_alpha = false;
    let mut has_digit = false;
    for c in s.chars() {
        if c.is_ascii_alphabetic() {
            has_alpha = true;
        } else if c.is_ascii_digit() {
            has_digit = true;
        } else if !matches!(c, '-' | '_' | '/' | '.') {
            return false;
        }
    }
    has_alpha && has_digit && s.len() <= 20
}

/// 目录名清洗为标题：去序号前缀、去标签型方括号、去两端杂散符号
pub fn clean_title(name: &str) -> String {
    let mut s = name.to_string();

    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < b.len() && matches!(b[i], b' ' | b'.' | b'-' | b'_') {
        s = s[i + 1..].trim_start().to_string();
    }

    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(p) = rest.find('[') {
        let (head, tail) = rest.split_at(p);
        let inner_end = match tail[1..].find(']') {
            Some(e) => 1 + e,
            None => break,
        };
        let inner = &tail[1..inner_end];
        out.push_str(head);
        if !is_tag_content(inner) {
            out.push('[');
            out.push_str(inner);
            out.push(']');
        }
        rest = &tail[inner_end + 1..];
    }
    out.push_str(rest);

    out.trim()
        .trim_matches(|c: char| c == ' ' || c == '-' || c == '_' || c == '.' || c == '　')
        .to_string()
}

fn is_tag_content(inner: &str) -> bool {
    let l = inner.to_lowercase();
    if l.is_empty() {
        return true;
    }
    // 品番类（KSLA-0098）视为标签
    if is_catalog_like(inner) {
        return true;
    }
    if l.chars().all(|c| c.is_ascii_digit()) {
        return l.len() >= 4;
    }
    if l.chars().filter(|c| c.is_ascii_digit()).count() >= 6 && l.contains('.') {
        return true;
    }
    const TAGS: &[&str] = &[
        "flac", "wav", "mp3", "cue", "log", "img", "image", "scan", "booklet", "lossless",
        "cd-", "+cue", "pdf", "jpg", "png", "zip", "rar", "7z", "dsf", "ape", "m4a", "ogg",
        "dvd", "bluray", "type-", "vol", "disc", "disk", "side", "章", "巻", "卷",
        "通常", "初回", "限定", "特装", "同梱", "通常盤", "特典",
    ];
    TAGS.iter().any(|t| l.contains(t))
}

// ---------------------------------------------------------------- 说明文件解析

fn find_note(idx: &FileIndex, unit_name: &str) -> Option<String> {
    let target = format!("{}.txt", unit_name);
    let top = idx.top_level();
    let mut candidates: Vec<&PathBuf> = Vec::new();
    for p in &top {
        let p = *p;
        if p.extension().map(|x| x.eq_ignore_ascii_case("txt")).unwrap_or(false) {
            if p.file_name().map(|n| n.to_string_lossy() == target).unwrap_or(false) {
                return read_head(p, 64 * 1024);
            }
            candidates.push(p);
        }
    }
    candidates.truncate(12);
    for p in candidates {
        if let Some(text) = read_head(p, 16 * 1024) {
            if text.contains("本信息文件由") && text.contains('【') {
                return Some(text);
            }
        }
    }
    None
}

fn find_readme(idx: &FileIndex) -> Option<String> {
    for p in idx.top_level() {
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        let is_readme = name.contains("readme")
            || name.contains("说明")
            || name.contains("介绍")
            || name.contains("简介")
            || name.contains("资料")
            || (name.starts_with("note") && name.ends_with(".txt"));
        if !is_readme {
            continue;
        }
        let text = read_head(p, 8 * 1024)?;
        let joined = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let joined = joined.replace('#', "");
        if joined.chars().count() > 6 && !joined.contains("本信息文件由") {
            return Some(shorten(&joined, 300));
        }
    }
    None
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

fn read_head(path: &Path, max: usize) -> Option<String> {
    Some(decode_bytes(&read_raw(path, max)?))
}

fn read_raw(path: &Path, max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let mut n = 0usize;
    while n < max {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => break,
        }
    }
    buf.truncate(n);
    Some(buf)
}

/// UTF-8（可带 BOM）→ GBK 兜底
pub fn decode_bytes(b: &[u8]) -> String {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&b[3..]).to_string();
    }
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    let (s, _, _) = encoding_rs::GBK.decode(b);
    s.into_owned()
}

/// 解析 `【标签】值` 行
pub fn parse_note(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix('【') {
            if let Some(end) = rest.find('】') {
                let key = rest[..end].to_string();
                let value = rest[end + '】'.len_utf8()..].trim().to_string();
                if !value.is_empty() && !is_placeholder(&value) {
                    map.entry(key).or_insert(value);
                }
            }
        }
    }
    map
}

/// 说明文件字段级对比的一行
#[derive(Debug, Clone, Serialize)]
pub struct FieldDiff {
    pub field: String,
    pub old: String,
    pub new: String,
    /// added | removed | changed | same
    pub status: String,
}

/// 解析 `【标签】值` 段落（保序；值可跨多行，直到下一个段 / 分隔线 / 页脚）
pub fn parse_note_blocks(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let flush = |out: &mut Vec<(String, String)>, cur: &mut Option<(String, Vec<String>)>| {
        if let Some((key, lines)) = cur.take() {
            let value = lines.join("\n").trim().to_string();
            if !value.is_empty() {
                out.push((key, value));
            }
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('【') {
            if let Some(end) = rest.find('】') {
                flush(&mut out, &mut current);
                let key = rest[..end].trim().to_string();
                if key.is_empty() {
                    continue;
                }
                let inline = rest[end + '】'.len_utf8()..].trim().to_string();
                current = Some((
                    key,
                    if inline.is_empty() {
                        Vec::new()
                    } else {
                        vec![inline]
                    },
                ));
                continue;
            }
        }
        if t.starts_with("本信息文件由") || t.starts_with('=') || t.starts_with('-') {
            flush(&mut out, &mut current);
            continue;
        }
        if let Some((_, buf)) = current.as_mut() {
            buf.push(line.to_string());
        }
    }
    flush(&mut out, &mut current);
    out
}

/// 以「重新渲染的结果」为主序做字段级 diff；旧文件独有的字段追加在末尾
pub fn diff_note_text(existing: &str, rendered: &str) -> Vec<FieldDiff> {
    let old_blocks = parse_note_blocks(existing);
    let new_blocks = parse_note_blocks(rendered);
    let old_map: HashMap<&str, &str> = old_blocks
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let new_map: HashMap<&str, &str> = new_blocks
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut out = Vec::new();
    for (k, nv) in &new_blocks {
        let status = match old_map.get(k.as_str()) {
            None => "added",
            Some(ov) if ov.trim() == nv.trim() => "same",
            Some(_) => "changed",
        };
        out.push(FieldDiff {
            field: k.clone(),
            old: old_map.get(k.as_str()).map(|s| s.to_string()).unwrap_or_default(),
            new: nv.to_string(),
            status: status.to_string(),
        });
    }
    for (k, ov) in &old_blocks {
        if new_map.contains_key(k.as_str()) {
            continue;
        }
        out.push(FieldDiff {
            field: k.clone(),
            old: ov.to_string(),
            new: String::new(),
            status: "removed".to_string(),
        });
    }
    out
}

/// 单个说明文件的核对结果（现有文件 vs 按当前条目重新渲染）
#[derive(Debug, Clone, Serialize)]
pub struct NoteAudit {
    /// 说明文件是否存在
    pub exists: bool,
    /// 新模板要求、但现有文件缺失的字段标签（即 diff 中的 added）
    pub missing: Vec<String>,
    /// 值与现有文件不一致的字段数
    pub changed: usize,
    /// 新模板有、旧文件无的字段数（= missing.len()，保留计数与 diff 一致）
    pub added: usize,
    /// 旧文件有、新模板不再包含的字段数（陈旧/未知字段）
    pub removed: usize,
    /// 值完全一致的字段数
    pub same: usize,
}

/// 批量核对用：existing 为 None（文件不存在）时只标记缺失，不做字段对比
pub fn audit_note(existing: Option<&str>, rendered: &str) -> NoteAudit {
    let Some(existing) = existing else {
        return NoteAudit {
            exists: false,
            missing: Vec::new(),
            changed: 0,
            added: 0,
            removed: 0,
            same: 0,
        };
    };
    let mut audit = NoteAudit {
        exists: true,
        missing: Vec::new(),
        changed: 0,
        added: 0,
        removed: 0,
        same: 0,
    };
    for d in diff_note_text(existing, rendered) {
        match d.status.as_str() {
            "added" => {
                audit.missing.push(d.field);
                audit.added += 1;
            }
            "changed" => audit.changed += 1,
            "removed" => audit.removed += 1,
            _ => audit.same += 1,
        }
    }
    audit
}

fn note_value(map: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = map.get(*k) {
            if !is_placeholder(v) {
                return Some(v.clone());
            }
        }
    }
    None
}

/// 从说明文件正文提取曲目行（`NN. 标题`，含多碟 `◆ 小节`）
pub fn note_tracklist_from_text(text: &str) -> Vec<String> {
    let mut tracks: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let l = line.trim();
        if l.contains("Tracklist") || l.contains("曲目列表") || l.contains("收录曲目") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if l.starts_with("【") || l.starts_with('=') || l.starts_with("本信息文件") {
            in_section = false;
            continue;
        }
        if l.starts_with('◆') {
            let name = l.trim_start_matches('◆').trim().to_string();
            if !name.is_empty() {
                tracks.push(format!("◆ {}", name));
            }
            continue;
        }
        if let Some(t) = numbered_line(l) {
            if tracks.last().map(|x| *x != t).unwrap_or(true) {
                tracks.push(t);
            }
        }
    }
    tracks
}

fn numbered_line(l: &str) -> Option<String> {
    let bytes = l.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i > 3 {
        return None;
    }
    let rest = l[i..].strip_prefix('.')?;
    let t = rest.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// ---------------------------------------------------------------- CUE / 文件名曲目

fn cue_tracklist(idx: &FileIndex) -> Option<Vec<String>> {
    for path in &idx.files {
        if path.extension().map(|x| x.eq_ignore_ascii_case("cue")).unwrap_or(false) {
            let text = read_head(path, 512 * 1024)?;
            let tracks = crate::notes::parse_cue(&text);
            if !tracks.is_empty() {
                return Some(tracks);
            }
        }
    }
    None
}

fn numbered_audio_names(idx: &FileIndex) -> Vec<String> {
    const AUDIO: &[&str] = &["flac", "mp3", "wav", "ape", "m4a", "ogg", "dsf", "wma"];
    let mut out = Vec::new();
    for path in &idx.files {
        let name = match path.file_name() {
            Some(n) => n.to_string_lossy().to_lowercase(),
            None => continue,
        };
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
        if !AUDIO.contains(&ext.as_str()) {
            continue;
        }
        let stem = name.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(name);
        if let Some(t) = numbered_line(&stem) {
            out.push(t);
        }
    }
    out
}

pub fn walk_limited(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max {
            break;
        }
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            if out.len() >= max {
                break;
            }
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------- 音频标签

#[derive(Debug, Default, Clone)]
pub struct AudioTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub date: String,
    pub label: String,
    pub catalog: String,
}

fn audio_file(idx: &FileIndex) -> Option<PathBuf> {
    const PREF: &[&str] = &["flac", "mp3", "wav", "ape", "m4a"];
    for pref in PREF {
        for p in &idx.files {
            if p.extension().map(|x| x.eq_ignore_ascii_case(pref)).unwrap_or(false) {
                return Some(p.clone());
            }
        }
    }
    None
}

pub fn read_tags(unit: &Path) -> AudioTags {
    read_tags_from(&FileIndex::new(unit))
}

fn read_tags_from(idx: &FileIndex) -> AudioTags {
    let path = match audio_file(idx) {
        Some(p) => p,
        None => return AudioTags::default(),
    };
    let ext = path
        .extension()
        .map(|x| x.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let head = read_raw(&path, 512 * 1024).unwrap_or_default();
    match ext.as_str() {
        "flac" => flac_tags(&head),
        "mp3" => id3_tags(&head),
        "wav" => wav_tags(&head),
        _ => AudioTags::default(),
    }
}

fn nonempty(s: String) -> Option<String> {
    let s = s.trim().to_string();
    if s.is_empty() || is_placeholder(&s) {
        None
    } else {
        Some(s)
    }
}

/// FLAC: magic + metadata blocks（type 4 = VORBIS_COMMENT）
fn flac_tags(b: &[u8]) -> AudioTags {
    let mut t = AudioTags::default();
    if b.len() < 8 || &b[..4] != b"fLaC" {
        return t;
    }
    let mut pos = 4usize;
    loop {
        if pos + 4 > b.len() {
            break;
        }
        let last = b[pos] & 0x80 != 0;
        let kind = b[pos] & 0x7F;
        let len = ((b[pos + 1] as usize) << 16) | ((b[pos + 2] as usize) << 8) | (b[pos + 3] as usize);
        let body_start = pos + 4;
        if body_start + len > b.len() {
            break;
        }
        if kind == 4 {
            parse_vorbis(&b[body_start..body_start + len], &mut t);
            break;
        }
        if last {
            break;
        }
        pos = body_start + len;
    }
    t
}

fn parse_vorbis(b: &[u8], t: &mut AudioTags) {
    let u32le = |buf: &[u8], at: usize| -> u32 {
        if at + 4 > buf.len() {
            return u32::MAX;
        }
        u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
    };
    let mut i = 0usize;
    let vendor_len = u32le(b, i) as usize;
    if vendor_len == u32::MAX as usize {
        return;
    }
    i += 4 + vendor_len;
    let count = u32le(b, i) as usize;
    if count == u32::MAX as usize {
        return;
    }
    i += 4;
    for _ in 0..count.min(256) {
        let len = u32le(b, i) as usize;
        if len == u32::MAX as usize || i + 4 + len > b.len() {
            break;
        }
        i += 4;
        let s = String::from_utf8_lossy(&b[i..i + len]).to_string();
        i += len;
        if let Some((k, v)) = s.split_once('=') {
            let v = v.trim().to_string();
            match k.trim().to_ascii_uppercase().as_str() {
                "TITLE" if t.title.is_empty() => t.title = v,
                "ARTIST" if t.artist.is_empty() => t.artist = v,
                "ALBUM" if t.album.is_empty() => t.album = v,
                "DATE" | "YEAR" if t.date.is_empty() => t.date = v,
                "LABEL" | "ORGANIZATION" | "PUBLISHER" if t.label.is_empty() => t.label = v,
                "CATALOGNUMBER" | "CATALOG" | "CATALOGUE" if t.catalog.is_empty() => t.catalog = v,
                _ => {}
            }
        }
    }
}

/// ID3v2.3 / v2.4 文本帧
fn id3_tags(b: &[u8]) -> AudioTags {
    let mut t = AudioTags::default();
    if b.len() < 10 || &b[..3] != b"ID3" {
        return t;
    }
    let ver = b[3];
    let size = syncsafe(&b[6..10]) as usize;
    let mut pos = 10usize;
    let end = (10 + size).min(b.len());
    let mut frames = 0usize;
    while pos + 10 <= end && frames < 80 {
        frames += 1;
        let id = &b[pos..pos + 4];
        if id[0] == 0 {
            break;
        }
        let fsize = if ver >= 4 {
            syncsafe(&b[pos + 4..pos + 8]) as usize
        } else {
            u32::from_be_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize
        };
        if fsize == 0 || pos + 10 + fsize > end {
            break;
        }
        let body = &b[pos + 10..pos + 10 + fsize];
        let id_str = String::from_utf8_lossy(id).to_string();
        match id_str.as_str() {
            "TIT2" => t.title = text_body(body),
            "TPE1" | "TPE2" => {
                if t.artist.is_empty() {
                    t.artist = text_body(body)
                }
            }
            "TALB" => t.album = text_body(body),
            "TDRC" | "TYER" | "TDRL" => t.date = text_body(body),
            "TXXX" => {
                if body.len() > 2 {
                    let enc = body[0];
                    let wide = enc == 1 || enc == 2;
                    let step = if wide { 2 } else { 1 };
                    let mut k = 1usize;
                    while k + step <= body.len() - 1 {
                        let hit = if wide {
                            body[k] == 0 && body[k + 1] == 0
                        } else {
                            body[k] == 0
                        };
                        if hit {
                            break;
                        }
                        k += step;
                    }
                    let desc = decode_bytes(&body[1..k.min(body.len())]).to_uppercase();
                    let val_start = (k + step).min(body.len());
                    let val = decode_bytes(&body[val_start..]).trim().to_string();
                    if desc.contains("LABEL") || desc.contains("PUBLISHER") {
                        t.label = val;
                    } else if desc.contains("CATALOG") {
                        t.catalog = val;
                    }
                }
            }
            _ => {}
        }
        pos += 10 + fsize;
    }
    t
}

fn text_body(body: &[u8]) -> String {
    if body.is_empty() {
        return String::new();
    }
    let enc = body[0];
    let raw = &body[1..];
    let s = match enc {
        1 | 2 => decode_utf16(raw),
        _ => decode_bytes(raw),
    };
    s.trim_matches('\0').trim().to_string()
}

/// UTF-16（带 BOM 或按字节序）→ UTF-8
fn decode_utf16(raw: &[u8]) -> String {
    let mut units: Vec<u16> = Vec::new();
    let (le, mut i) = if raw.starts_with(&[0xFF, 0xFE]) {
        (true, 2)
    } else if raw.starts_with(&[0xFE, 0xFF]) {
        (false, 2)
    } else {
        (true, 0)
    };
    while i + 1 < raw.len() {
        let u = if le {
            u16::from_le_bytes([raw[i], raw[i + 1]])
        } else {
            u16::from_be_bytes([raw[i], raw[i + 1]])
        };
        units.push(u);
        i += 2;
    }
    char::decode_utf16(units)
        .filter_map(|c| c.ok())
        .collect::<String>()
}

fn syncsafe(b: &[u8]) -> u32 {
    if b.len() < 4 {
        return 0;
    }
    ((b[0] as u32 & 0x7F) << 21) | ((b[1] as u32 & 0x7F) << 14) | ((b[2] as u32 & 0x7F) << 7) | (b[3] as u32 & 0x7F)
}

/// WAV: LIST-INFO 子块
fn wav_tags(b: &[u8]) -> AudioTags {
    let mut t = AudioTags::default();
    let mut i = 12usize;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        if body + size > b.len() {
            break;
        }
        if id == b"LIST" && size >= 4 && &b[body..body + 4] == b"INFO" {
            let mut j = body + 4;
            while j + 8 <= body + size {
                let sub = &b[j..j + 4];
                let len = u32::from_le_bytes([b[j + 4], b[j + 5], b[j + 6], b[j + 7]]) as usize;
                if j + 8 + len > body + size {
                    break;
                }
                let v = decode_bytes(&b[j + 8..j + 8 + len]).trim_matches('\0').trim().to_string();
                if !v.is_empty() {
                    match sub {
                        b"INAM" => t.title = v,
                        b"IART" => t.artist = v,
                        b"ICRD" => t.date = v,
                        b"IPRD" => t.album = v,
                        b"ICMS" => t.label = v,
                        _ => {}
                    }
                }
                j += 8 + len + (len & 1);
            }
        }
        i = body + size + (size & 1);
    }
    t
}

// ---------------------------------------------------------------- 文件构成标签

fn composition_tags(idx: &FileIndex, entry: &Entry) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut image_exts: Vec<String> = Vec::new();
    let mut has_image = false;
    let mut has_audio = false;

    for path in &idx.files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
        match ext.as_str() {
            "flac" | "mp3" | "wav" | "ape" | "dsf" | "m4a" | "ogg" | "opus" | "wma" => {
                has_audio = true;
                if !tokens.contains(&ext) {
                    tokens.push(ext);
                }
            }
            "cue" | "log" => {
                if !tokens.contains(&ext) {
                    tokens.push(ext);
                }
            }
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" => {
                has_image = true;
                image_exts.push(ext);
            }
            "pdf" | "epub" | "azw3" | "mobi" | "doc" | "docx" | "cbz" | "cbr" | "zip" | "rar"
            | "7z" | "mp4" | "mkv" | "avi" | "lrc" | "ass" | "srt" | "psd" | "ai" => {
                if !tokens.contains(&ext) {
                    tokens.push(ext);
                }
            }
            _ => {}
        }
    }
    if has_image {
        if has_audio || entry.kind == "album" {
            if !tokens.contains(&"img".to_string()) {
                tokens.push("img".to_string());
            }
        } else {
            image_exts.sort();
            for e in image_exts {
                if !tokens.contains(&e) {
                    tokens.push(e);
                }
            }
        }
    }
    tokens.truncate(8);
    tokens
}

// ---------------------------------------------------------------- 封面提取

/// 单位封面（mime, bytes）。优先级：命名图 > 内嵌（FLAC PICTURE / ID3 APIC）> 目录内最大图片。
pub fn cover_bytes(unit: &Path) -> Option<(String, Vec<u8>)> {
    if !unit.is_dir() {
        return None;
    }
    // 1) 命名图（单位根目录直接子文件，不区分大小写，按优先序取首个可读）
    let mut root_files: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(rd) = fs::read_dir(unit) {
        root_files = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if !p.is_file() {
                    return None;
                }
                let n = p.file_name()?.to_string_lossy().to_lowercase();
                Some((n, p))
            })
            .collect();
    }
    const NAMES: &[&str] = &["cover", "folder", "front", "封面"];
    const NAMED_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
    for name in NAMES {
        for ext in NAMED_EXTS {
            let want = format!("{name}.{ext}");
            if let Some((_, p)) = root_files.iter().find(|(n, _)| *n == want) {
                if let Some(bytes) = read_cover_file(p) {
                    return Some((mime_of(&bytes).to_string(), bytes));
                }
            }
        }
    }
    // 2) 内嵌封面（根目录音频；flac 优先于 mp3）
    let mut audios: Vec<PathBuf> = root_files
        .iter()
        .filter(|(n, _)| n.ends_with(".flac") || n.ends_with(".mp3"))
        .map(|(_, p)| p.clone())
        .collect();
    audios.sort();
    audios.sort_by_key(|p| {
        if p.extension().map(|e| e.eq_ignore_ascii_case("flac")).unwrap_or(false) {
            0
        } else {
            1
        }
    });
    for p in audios {
        let b = read_raw(&p, 16 * 1024 * 1024).unwrap_or_default();
        if b.is_empty() {
            continue;
        }
        let img = flac_picture(&b).or_else(|| id3_apic(&b));
        if let Some(bytes) = img {
            if !bytes.is_empty() {
                return Some((mime_of(&bytes).to_string(), bytes));
            }
        }
    }
    // 3) 兜底：目录内最大图片
    const ALL_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff"];
    let mut best: Option<(u64, PathBuf)> = None;
    for p in walk_limited(unit, 500) {
        let ok = p
            .extension()
            .map(|e| ALL_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
            .unwrap_or(false);
        if !ok {
            continue;
        }
        let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if len == 0 || len > 30 * 1024 * 1024 {
            continue;
        }
        if best.as_ref().map(|(l, _)| len > *l).unwrap_or(true) {
            best = Some((len, p));
        }
    }
    let (_, p) = best?;
    let bytes = read_cover_file(&p)?;
    Some((mime_of(&bytes).to_string(), bytes))
}

fn read_cover_file(p: &Path) -> Option<Vec<u8>> {
    const MAX: u64 = 30 * 1024 * 1024;
    let len = fs::metadata(p).ok()?.len();
    if len == 0 || len > MAX {
        return None;
    }
    read_raw(p, MAX as usize)
}

/// 仅取内嵌封面（FLAC PICTURE 优先、ID3 APIC 次之；根目录音频，最多试 3 个）。
/// 不走命名图/目录图兜底 —— 供 `kindx` 做「歌曲 + 散图」封面交叉比对。
pub fn embedded_cover(unit: &Path) -> Option<Vec<u8>> {
    if !unit.is_dir() {
        return None;
    }
    let mut audios: Vec<PathBuf> = fs::read_dir(unit)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !p.is_file() {
                return None;
            }
            let n = p.file_name()?.to_string_lossy().to_lowercase();
            if n.ends_with(".flac") || n.ends_with(".mp3") {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    audios.sort_by_key(|p| {
        if p.extension().map(|e| e.eq_ignore_ascii_case("flac")).unwrap_or(false) {
            0
        } else {
            1
        }
    });
    for p in audios.into_iter().take(3) {
        let b = read_raw(&p, 16 * 1024 * 1024).unwrap_or_default();
        if b.is_empty() {
            continue;
        }
        if let Some(img) = flac_picture(&b).or_else(|| id3_apic(&b)) {
            if !img.is_empty() {
                return Some(img);
            }
        }
    }
    None
}

fn mime_of(b: &[u8]) -> &'static str {
    if b.len() >= 3 && b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF {
        "image/jpeg"
    } else if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        "image/png"
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        "image/gif"
    } else if b.starts_with(b"BM") {
        "image/bmp"
    } else if b.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || b.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        "image/tiff"
    } else {
        "application/octet-stream"
    }
}

/// FLAC metadata block type 6 = PICTURE（从文件头遍历，返回图片字节）
fn flac_picture(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 8 || &b[..4] != b"fLaC" {
        return None;
    }
    let mut pos = 4usize;
    let mut blocks = 0usize;
    while pos + 4 <= b.len() && blocks < 64 {
        blocks += 1;
        let last = b[pos] & 0x80 != 0;
        let kind = b[pos] & 0x7F;
        let len = ((b[pos + 1] as usize) << 16) | ((b[pos + 2] as usize) << 8) | (b[pos + 3] as usize);
        let body = pos + 4;
        if body + len > b.len() {
            break;
        }
        if kind == 6 {
            return parse_picture_block(&b[body..body + len]);
        }
        if last {
            break;
        }
        pos = body + len;
    }
    None
}

/// PICTURE block：type(4) + mimeLen(4) + mime + descLen(4) + desc + w/h/depth/colors(16) + dataLen(4) + data
fn parse_picture_block(b: &[u8]) -> Option<Vec<u8>> {
    let be32 = |at: usize| -> Option<usize> {
        if at + 4 > b.len() {
            return None;
        }
        Some(u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize)
    };
    let mut i = 4usize;
    let mime_len = be32(i)?;
    i += 4 + mime_len;
    let desc_len = be32(i)?;
    i += 4 + desc_len;
    i += 16;
    let data_len = be32(i)?;
    i += 4;
    if data_len == 0 || i + data_len > b.len() {
        return None;
    }
    Some(b[i..i + data_len].to_vec())
}

/// ID3v2.3 / v2.4 APIC 帧 → 图片字节
fn id3_apic(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 10 || &b[..3] != b"ID3" {
        return None;
    }
    let ver = b[3];
    let size = syncsafe(&b[6..10]) as usize;
    let mut pos = 10usize;
    let end = (10 + size).min(b.len());
    let mut frames = 0usize;
    while pos + 10 <= end && frames < 64 {
        frames += 1;
        let id = &b[pos..pos + 4];
        if id[0] == 0 {
            break;
        }
        let fsize = if ver >= 4 {
            syncsafe(&b[pos + 4..pos + 8]) as usize
        } else {
            u32::from_be_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize
        };
        if fsize == 0 || pos + 10 + fsize > end {
            break;
        }
        if id == b"APIC" {
            return apic_image(&b[pos + 10..pos + 10 + fsize]);
        }
        pos += 10 + fsize;
    }
    None
}

/// APIC body：encoding + mime\0 + picType + 描述(按 encoding 终止) + 图片数据
fn apic_image(body: &[u8]) -> Option<Vec<u8>> {
    if body.len() < 4 {
        return None;
    }
    let enc = body[0];
    let mut i = 1usize;
    let mstart = i;
    while i < body.len() && body[i] != 0 {
        i += 1;
        if i - mstart > 64 {
            return None;
        }
    }
    if i >= body.len() {
        return None;
    }
    i += 1;
    if i >= body.len() {
        return None;
    }
    i += 1; // 图片类型字节
    if i >= body.len() {
        return None;
    }
    let wide = enc == 1 || enc == 3;
    let mut found = false;
    if wide {
        while i + 1 < body.len() {
            if body[i] == 0 && body[i + 1] == 0 {
                i += 2;
                found = true;
                break;
            }
            i += 2;
        }
    } else {
        while i < body.len() {
            if body[i] == 0 {
                i += 1;
                found = true;
                break;
            }
            i += 1;
        }
    }
    if !found || i >= body.len() {
        return None;
    }
    Some(body[i..].to_vec())
}

/// 供 UI 展示的字段清单
pub fn field_labels() -> Vec<(String, String)> {
    FIELDS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}
