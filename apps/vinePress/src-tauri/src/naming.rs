//! 命名合法性检查与修复建议（网盘/本地规则，纯逻辑，零 Tauri 依赖）。
//!
//! 规则来源（web 调研结论，2026-06）：
//! - 各网盘核心非法集 `\/:*?"<>|` + 保留名 CON/PRN/AUX/NUL/COM1-9/LPT1-9（含扩展名形态）
//!   + 首尾空格/点 + 单段 255 字符；百度网盘文件夹首尾空格直接拒绝；
//!   115 下载时把特殊字符静默转 `_`；对象存储（腾讯 COS 风格）不推荐
//!   `` ` ^ { } [ ] ~ % # `` 且单键 ≤850 字节。
//! - 本模块只做**检测与建议名**，实际改名走 `fsops::rename_path`（根内校验 + 保留名复检）。

use crate::fsops::RESERVED_NAMES;
use crate::scan::DirNode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ---------------------------------------------------------------- 档案

/// 命名规则档案（按目标平台/网盘）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Windows 本地/同步盘（OneDrive 等）
    Windows,
    Baidu,
    Aliyun,
    Tianyi,
    /// 通用网盘（115/夸克…无严格文档，按核心非法集从严）
    Common,
    /// 对象存储 / 腾讯云 COS 风格（额外不推荐字符）
    Strict,
}

/// 档案表（id, 显示名）——前端下拉的唯一数据来源（经 `profile_options`）
pub const PROFILES: &[(&str, &str)] = &[
    ("windows", "Windows 本地 / 同步盘"),
    ("baidu", "百度网盘"),
    ("aliyun", "阿里云盘"),
    ("tianyi", "天翼云盘"),
    ("common", "通用网盘（115 / 夸克等）"),
    ("strict", "对象存储 / 腾讯云 COS"),
];

pub fn profile_from(s: &str) -> Profile {
    match s {
        "baidu" => Profile::Baidu,
        "aliyun" => Profile::Aliyun,
        "tianyi" => Profile::Tianyi,
        "common" => Profile::Common,
        "strict" => Profile::Strict,
        _ => Profile::Windows,
    }
}

pub fn profile_id(p: Profile) -> &'static str {
    match p {
        Profile::Windows => "windows",
        Profile::Baidu => "baidu",
        Profile::Aliyun => "aliyun",
        Profile::Tianyi => "tianyi",
        Profile::Common => "common",
        Profile::Strict => "strict",
    }
}

/// 档案是否云盘（前导空格在云盘为 error、本地仅 warn）
fn is_cloud(p: Profile) -> bool {
    p != Profile::Windows
}

// ---------------------------------------------------------------- 字符表

const WIN_ILLEGAL: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
const STRICT_WARN: &[char] = &['`', '^', '{', '}', '[', ']', '~', '%', '#'];

fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{00AD}'
    )
}

// ---------------------------------------------------------------- 检测结果

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameIssue {
    /// 相对根目录的路径（`/` 分隔）；根节点为空串
    pub rel_path: String,
    /// dir | file | root
    pub kind: String,
    pub name: String,
    /// 建议新名；与 `name` 相同 = 无法自动修复
    pub new_name: String,
    /// error（违规）| warn（提示）
    pub level: String,
    pub reasons: Vec<String>,
}

impl NameIssue {
    /// 能否一键修复（勾选框启用条件）
    pub fn fixable(&self) -> bool {
        self.kind != "root" && self.new_name != self.name
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameCheckResult {
    pub profile: String,
    pub issues: Vec<NameIssue>,
    pub checked_dirs: u32,
    pub checked_files: u32,
}

// ---------------------------------------------------------------- 名称级检查

/// 返回 (是否 error, 说明) 列表
fn check_name(name: &str, profile: Profile) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, String)> = Vec::new();
    if name.is_empty() {
        out.push((true, "名称为空".into()));
        return out;
    }
    if name == "." || name == ".." {
        out.push((true, "特殊目录名".into()));
        return out;
    }
    let illegal: Vec<char> = name.chars().filter(|c| WIN_ILLEGAL.contains(c)).collect();
    if !illegal.is_empty() {
        let list: String = illegal.iter().map(|c| format!("「{}」", c)).collect();
        out.push((
            true,
            format!("含非法字符{}（Windows 与各网盘上传均拒绝）", list),
        ));
    }
    if name.chars().any(|c| c.is_control()) {
        out.push((true, "含控制字符".into()));
    }
    let invis = name.chars().filter(|c| is_invisible(*c)).count();
    if invis > 0 {
        out.push((
            true,
            format!("含 {} 个零宽/不可见字符（肉眼难察觉，跨端会乱码）", invis),
        ));
    }
    let stem = name.split('.').next().unwrap_or(name).to_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        out.push((
            true,
            format!("「{}」是 Windows/网盘保留名（含其扩展名形态）", stem.to_uppercase()),
        ));
    }
    if name.ends_with(' ') {
        out.push((true, "以空格结尾（网盘同步/下载会出错）".into()));
    }
    if name.ends_with('.') {
        out.push((true, "以点结尾（Windows 拒绝）".into()));
    }
    if name.starts_with(' ') {
        out.push((
            is_cloud(profile),
            "以空格开头（网盘会拒绝或静默截断）".into(),
        ));
    }
    if name.encode_utf16().count() > 255 {
        out.push((true, "名称过长（>255 字符）".into()));
    }
    if profile == Profile::Strict {
        let warn: Vec<char> = name.chars().filter(|c| STRICT_WARN.contains(c)).collect();
        if !warn.is_empty() {
            let list: String = warn.iter().map(|c| format!("「{}」", c)).collect();
            out.push((
                false,
                format!("含对象存储不推荐字符{}（COS 等建议避开）", list),
            ));
        }
    }
    out
}

// ---------------------------------------------------------------- 建议名

/// 生成可提交的修复名（不保证同级唯一，由调用方 dedupe）
pub fn sanitize(name: &str, profile: Profile) -> String {
    let mut s: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_invisible(*c))
        .map(|c| match c {
            ':' => '：',
            '*' => '＊',
            '?' => '？',
            '<' => '＜',
            '>' => '＞',
            '|' => '｜',
            '"' => '＂',
            '/' => '／',
            '\\' => '＼',
            _ => c,
        })
        .collect();
    if profile == Profile::Strict {
        s = s
            .chars()
            .map(|c| match c {
                '`' => '｀',
                '^' => '＾',
                '{' => '｛',
                '}' => '｝',
                '[' => '［',
                ']' => '］',
                '~' => '～',
                '%' => '％',
                '#' => '＃',
                _ => c,
            })
            .collect();
    }
    let mut s = s.trim().to_string();
    while s.ends_with('.') || s.ends_with(' ') || s.ends_with('\u{3000}') {
        s.pop();
    }
    if s.is_empty() {
        s = "未命名".to_string();
    }
    // 保留名：在首点前插下划线（NUL.txt → NUL_.txt）
    let stem_end = s.find('.').unwrap_or(s.len());
    let stem = s[..stem_end].to_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        s.insert(stem_end, '_');
    }
    if s.encode_utf16().count() > 255 {
        s = truncate_utf16(&s, 255);
    }
    s
}

/// 保扩展名的 UTF-16 长度截断（Windows 计数以 UTF-16 code unit 为准）
fn truncate_utf16(s: &str, max: usize) -> String {
    let ext = match s.rfind('.') {
        Some(i) if i > 0 => {
            let e = &s[i..];
            if e.len() <= 9 && e[1..].chars().all(|c| c.is_ascii_alphanumeric()) {
                e
            } else {
                ""
            }
        }
        _ => "",
    };
    let stem = &s[..s.len() - ext.len()];
    let mut out = stem.to_string();
    while out.encode_utf16().count() + ext.encode_utf16().count() > max {
        if out.pop().is_none() {
            break;
        }
    }
    format!("{}{}", out, ext)
}

/// 同级去重：候选与既有名（大小写不敏感）冲突时追加 " (2)"、" (3)"…
/// `original_ci` = 待改名项自身的原名（小写）；候选 == 原名时视为无变化直接放行。
fn dedupe(candidate: String, original_ci: &str, taken: &mut HashSet<String>) -> String {
    let cand_ci = candidate.to_lowercase();
    if cand_ci == original_ci || !taken.contains(&cand_ci) {
        taken.insert(cand_ci);
        return candidate;
    }
    let (stem, ext) = split_ext(&candidate);
    let mut n = 2;
    loop {
        let alt = format!("{} ({}){}", stem, n, ext);
        let alt_ci = alt.to_lowercase();
        if !taken.contains(&alt_ci) {
            taken.insert(alt_ci);
            return alt;
        }
        n += 1;
    }
}

/// 拆「主名 + 扩展名」（扩展名 ≤8 个 ASCII 字母数字才算）
fn split_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => {
            let ext = &name[i..];
            if ext.len() <= 9 && ext[1..].chars().all(|c| c.is_ascii_alphanumeric()) {
                (name[..i].to_string(), ext.to_string())
            } else {
                (name.to_string(), String::new())
            }
        }
        _ => (name.to_string(), String::new()),
    }
}

// ---------------------------------------------------------------- 树级检查

/// 检查整棵目录树（含根节点、所有目录与文件名；不读盘，只看 `DirNode`）
pub fn check_tree(root: &DirNode, profile: Profile) -> NameCheckResult {
    let mut issues = Vec::new();
    let mut dirs = 1u32; // 根
    let mut files = 0u32;
    // 根不可在应用内改名 → 建议名保持原样
    let mut root_reasons = check_name(&root.name, profile);
    if let Some(r) = path_len_reason(&root.rel_path) {
        root_reasons.push(r);
    }
    push_issue(
        &mut issues,
        String::new(),
        "root",
        &root.name,
        &root_reasons,
        root.name.clone(),
    );
    walk(root, profile, &mut issues, &mut dirs, &mut files);
    NameCheckResult {
        profile: profile_id(profile).to_string(),
        issues,
        checked_dirs: dirs,
        checked_files: files,
    }
}

fn push_issue(
    out: &mut Vec<NameIssue>,
    rel: String,
    kind: &str,
    name: &str,
    reasons: &[(bool, String)],
    new_name: String,
) {
    if reasons.is_empty() {
        return;
    }
    let level = if reasons.iter().any(|r| r.0) {
        "error"
    } else {
        "warn"
    };
    out.push(NameIssue {
        rel_path: rel,
        kind: kind.to_string(),
        name: name.to_string(),
        new_name,
        level: level.to_string(),
        reasons: reasons.iter().map(|r| r.1.clone()).collect(),
    });
}

/// rel 超长警告（相对路径 UTF-16 超 260 → Windows MAX_PATH/网盘易失败）
fn path_len_reason(rel: &str) -> Option<(bool, String)> {
    let len = rel.encode_utf16().count();
    if len > 260 {
        Some((
            false,
            format!("相对路径过长（{} > 260 字符），上传网盘可能失败", len),
        ))
    } else {
        None
    }
}

fn walk(
    node: &DirNode,
    profile: Profile,
    out: &mut Vec<NameIssue>,
    dirs: &mut u32,
    files: &mut u32,
) {
    // 同级命名空间：本目录的子目录 + 直属文件共享（Windows 目录项不分类型）。
    // 注意不含 node 自身名——父目录名与子项同名（foo/foo）在磁盘上是合法的。
    let mut taken: HashSet<String> = HashSet::new();
    for c in &node.children {
        taken.insert(c.name.to_lowercase());
    }
    for f in &node.files {
        taken.insert(f.name.to_lowercase());
    }

    for c in &node.children {
        let mut reasons = check_name(&c.name, profile);
        if let Some(r) = path_len_reason(&c.rel_path) {
            reasons.push(r);
        }
        let mut new_name = sanitize(&c.name, profile);
        if new_name != c.name {
            new_name = dedupe(new_name, &c.name.to_lowercase(), &mut taken);
            taken.remove(&c.name.to_lowercase());
        }
        push_issue(
            out,
            c.rel_path.clone(),
            "dir",
            &c.name,
            &reasons,
            new_name,
        );
        *dirs += 1;
        walk(c, profile, out, dirs, files);
    }

    for f in &node.files {
        *files += 1;
        let reasons = check_name(&f.name, profile);
        if reasons.is_empty() {
            continue;
        }
        let rel = if node.rel_path.is_empty() {
            f.name.clone()
        } else {
            format!("{}/{}", node.rel_path, f.name)
        };
        let mut new_name = sanitize(&f.name, profile);
        if new_name != f.name {
            new_name = dedupe(new_name, &f.name.to_lowercase(), &mut taken);
            taken.remove(&f.name.to_lowercase());
        }
        push_issue(out, rel, "file", &f.name, &reasons, new_name);
    }
}

/// 档案下拉项（id, 显示名）序列化给前端
pub fn profile_options() -> Vec<(String, String)> {
    PROFILES
        .iter()
        .map(|(id, label)| (id.to_string(), label.to_string()))
        .collect()
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::FileStat;

    fn dir(name: &str, rel: &str) -> DirNode {
        DirNode {
            name: name.into(),
            rel_path: rel.into(),
            files: Vec::new(),
            children: Vec::new(),
            total_size: 0,
            total_files: 0,
        }
    }

    fn file(name: &str) -> FileStat {
        FileStat {
            name: name.into(),
            size: 1,
        }
    }

    #[test]
    fn sanitize_illegal_to_fullwidth() {
        assert_eq!(sanitize("专辑:Disc1", Profile::Windows), "专辑：Disc1");
        assert_eq!(sanitize("a?b*c", Profile::Common), "a？b＊c");
        assert_eq!(sanitize("a/b\\c", Profile::Windows), "a／b＼c");
        assert_eq!(sanitize("\"引号\"", Profile::Windows), "＂引号＂");
    }

    #[test]
    fn sanitize_reserved_and_edges() {
        assert_eq!(sanitize("NUL.txt", Profile::Windows), "NUL_.txt");
        assert_eq!(sanitize("con", Profile::Baidu), "con_");
        assert_eq!(sanitize("名 ", Profile::Windows), "名");
        assert_eq!(sanitize("名.", Profile::Windows), "名");
        assert_eq!(sanitize("  ", Profile::Windows), "未命名");
        assert_eq!(sanitize("\u{200B}隐藏", Profile::Windows), "隐藏");
        assert_eq!(sanitize("  前导空格", Profile::Windows), "前导空格");
    }

    #[test]
    fn sanitize_truncates_long_name_keeping_ext() {
        let long = format!("{}.flac", "长".repeat(300));
        let out = sanitize(&long, Profile::Windows);
        assert!(out.encode_utf16().count() <= 255);
        assert!(out.ends_with(".flac"));
        assert!(out.starts_with("长"));
    }

    #[test]
    fn sanitize_strict_maps_warn_chars_only_in_strict() {
        assert_eq!(sanitize("a#b", Profile::Strict), "a＃b");
        assert_eq!(sanitize("a#b", Profile::Baidu), "a#b");
    }

    #[test]
    fn check_levels() {
        // 非法字符 = error（任何档案）
        for p in [Profile::Windows, Profile::Baidu, Profile::Strict] {
            let r = check_name("a:b", p);
            assert!(r.iter().any(|x| x.0 && x.1.contains("非法字符")), "{:?}", p);
        }
        // 前导空格：本地 warn、云盘 error
        let win = check_name(" x", Profile::Windows);
        assert!(win.iter().any(|x| !x.0 && x.1.contains("空格开头")));
        let cloud = check_name(" x", Profile::Baidu);
        assert!(cloud.iter().any(|x| x.0 && x.1.contains("空格开头")));
        // 保留名
        let r = check_name("nul.txt", Profile::Windows);
        assert!(r.iter().any(|x| x.0 && x.1.contains("保留名")));
        // strict 档位专用 warn
        let r = check_name("a#b", Profile::Strict);
        assert!(r.iter().any(|x| !x.0 && x.1.contains("不推荐")));
        assert!(check_name("a#b", Profile::Baidu).is_empty());
        // 正常名无问题
        assert!(check_name("专辑 Disc 01", Profile::Baidu).is_empty());
    }

    #[test]
    fn check_tree_walks_dirs_files_root() {
        let mut root = dir("根目录", "");
        root.files.push(file("root note.txt"));
        let mut bad = dir("坏:目录", "坏:目录");
        bad.files.push(file("nul.txt"));
        bad.files.push(file("ok.txt"));
        root.children.push(bad);
        root.children.push(dir("好目录", "好目录"));

        let res = check_tree(&root, Profile::Baidu);
        // 根 + 2 个子目录
        assert_eq!(res.checked_dirs, 3);
        assert_eq!(res.checked_files, 3);
        // 2 个错误：坏:目录、nul.txt；根无问题
        assert_eq!(res.issues.len(), 2);
        let d = res.issues.iter().find(|i| i.kind == "dir").unwrap();
        assert_eq!(d.rel_path, "坏:目录");
        assert_eq!(d.new_name, "坏：目录");
        assert_eq!(d.level, "error");
        let f = res.issues.iter().find(|i| i.kind == "file").unwrap();
        assert_eq!(f.new_name, "nul_.txt");
        assert!(f.fixable());
        assert!(res.issues.iter().all(|i| i.kind != "root"));
    }

    #[test]
    fn check_tree_dedupes_suggested_names() {
        let mut root = dir("根", "");
        root.children.push(dir("a:b", "a:b"));
        root.children.push(dir("a：b", "a：b"));
        let res = check_tree(&root, Profile::Windows);
        let fixed: Vec<&NameIssue> = res.issues.iter().filter(|i| i.fixable()).collect();
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0].name, "a:b");
        assert_eq!(fixed[0].new_name, "a：b (2)");
    }

    #[test]
    fn check_tree_root_unfixable_and_path_len() {
        let mut root = dir("根:", "");
        // 深层 rel 超长 → 目录行附 path warn（且不可修）
        let long_rel = "a".repeat(261);
        let deep = dir("x", &long_rel);
        root.children.push(deep);
        let res = check_tree(&root, Profile::Windows);
        let root_issue = res.issues.iter().find(|i| i.kind == "root").unwrap();
        assert!(!root_issue.fixable());
        assert_eq!(root_issue.new_name, root_issue.name);
        let deep_issue = res.issues.iter().find(|i| i.rel_path == long_rel).unwrap();
        assert_eq!(deep_issue.level, "warn");
        assert!(!deep_issue.fixable());
        assert!(deep_issue.reasons.iter().any(|r| r.contains("相对路径过长")));
    }

    #[test]
    fn profile_roundtrip() {
        for (id, _) in PROFILES {
            assert_eq!(profile_id(profile_from(id)), *id);
        }
        assert_eq!(profile_from("nope"), Profile::Windows);
        assert_eq!(profile_options().len(), PROFILES.len());
    }
}
