//! 文件系统操作：浏览、打开、定位、新建、复制、移动。
//! 所有写操作与浏览都必须落在「项目根目录」内（路径前缀校验）。

use serde::Serialize;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Debug, Clone, Serialize)]
pub struct FileItem {
    pub name: String,
    /// 绝对路径
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: u64,
    pub ext: String,
}

// ---------------------------------------------------------------- 路径校验

fn canon(s: &str) -> String {
    s.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// 校验 path 位于 root 之内（含 root 本身），拒绝 `..`
pub fn ensure_under(root: &str, path: &str) -> Result<PathBuf, String> {
    if root.trim().is_empty() {
        return Err("未设置项目根目录，请先扫描根目录".into());
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!("操作被拒绝：路径包含 ..（{}）", path));
    }
    let r = canon(root);
    let t = canon(path);
    if t != r && !t.starts_with(&format!("{}\\", r)) {
        return Err(format!("操作被拒绝：路径不在项目根目录内（{}）", path));
    }
    Ok(PathBuf::from(path))
}

/// 校验「相对根目录的路径」（条目 rel_path）：拒绝 `..`、盘符、反斜杠、绝对路径
pub fn validate_rel(rel: &str) -> Result<(), String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("相对路径为空".into());
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(format!("操作被拒绝：不是相对路径（{}）", rel));
    }
    if rel.contains('\\') {
        return Err(format!("操作被拒绝：路径含反斜杠（{}）", rel));
    }
    if rel.contains(':') {
        return Err(format!("操作被拒绝：路径含盘符（{}）", rel));
    }
    for seg in rel.split('/') {
        if seg == ".." || seg == "." {
            return Err(format!("操作被拒绝：路径包含 {}（{}）", seg, rel));
        }
    }
    Ok(())
}

/// dest 是否位于 src 目录内部（含 src 自身）
fn is_inside(src: &Path, dest: &Path) -> bool {
    let s = canon(&src.to_string_lossy());
    let d = canon(&dest.to_string_lossy());
    d == s || d.starts_with(&format!("{}\\", s))
}

/// 复制/移动防护：目标不能位于源目录内部（否则会复制进自身子目录）
pub fn ensure_not_inside(dest: &Path, src: &Path) -> Result<(), String> {
    if src.is_dir() && is_inside(src, dest) {
        return Err(format!(
            "不能复制/移动到自身子目录（{} → {}）",
            src.display(),
            dest.display()
        ));
    }
    Ok(())
}

/// 是否为重联结点（symlink / junction），遍历时一律跳过
pub fn is_reparse(meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        meta.file_type().is_symlink()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
}

// ---------------------------------------------------------------- 浏览

pub fn list_dir(root: &str, path: &str, show_all: bool) -> Result<Vec<FileItem>, String> {
    let dir = ensure_under(root, path)?;
    if !dir.is_dir() {
        return Err(format!("不是目录：{}", path));
    }
    let rd = fs::read_dir(&dir).map_err(|e| format!("读取失败：{}", e))?;
    let mut items = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !show_all && (name.starts_with('.') || is_system_file(&name)) {
            continue;
        }
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let p = e.path();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        items.push(FileItem {
            name,
            path: p.to_string_lossy().to_string(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            mtime_ms: mtime,
            ext: p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        });
    }
    items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(items)
}

fn is_system_file(name: &str) -> bool {
    let l = name.to_lowercase();
    l == "desktop.ini" || l == "thumbs.db" || l == ".ds_store"
}

// ---------------------------------------------------------------- 打开 / 定位

pub fn open_path(root: &str, path: &str) -> Result<(), String> {
    let p = ensure_under(root, path)?;
    if !p.exists() {
        return Err(format!("路径不存在：{}", path));
    }
    // ShellExecuteExW：不经 shell 拼接，文件名含 & 等元字符也安全
    opener::open(&p).map_err(|e| format!("打开失败：{}", e))
}

pub fn reveal_path(root: &str, path: &str) -> Result<(), String> {
    let p = ensure_under(root, path)?;
    if !p.exists() {
        return Err(format!("路径不存在：{}", path));
    }
    Command::new("explorer")
        .arg(format!("/select,{}", p.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("定位失败：{}", e))?;
    Ok(())
}

// ---------------------------------------------------------------- 新建

pub fn make_directory(root: &str, path: &str) -> Result<(), String> {
    let p = ensure_under(root, path)?;
    if p.exists() {
        return Err(format!("已存在：{}", path));
    }
    fs::create_dir_all(&p).map_err(|e| format!("创建失败：{}", e))
}

// ---------------------------------------------------------------- 重命名

pub(crate) const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// 计算 path 相对 root 的路径（`/` 分隔、保留大小写）；不在根内返回 None
pub fn relative_to(root: &str, path: &str) -> Option<String> {
    let r: Vec<String> = root
        .trim()
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    let segs: Vec<&str> = path
        .trim()
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    if r.is_empty() || segs.len() < r.len() {
        return None;
    }
    let low: Vec<String> = segs.iter().map(|s| s.to_lowercase()).collect();
    if low[..r.len()] != r[..] {
        return None;
    }
    if segs.len() == r.len() {
        return Some(String::new());
    }
    Some(segs[r.len()..].join("/"))
}

/// 校验新名称（Windows 语义）
fn validate_new_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("新名称不能为空".into());
    }
    if name == "." || name == ".." {
        return Err("名称不合法".into());
    }
    if name.contains(['/', '\\']) {
        return Err("名称不能包含路径分隔符".into());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("名称不能包含控制字符".into());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err(format!("名称不能包含非法字符 {}", bad));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("名称不能以点或空格结尾".into());
    }
    if name.chars().count() > 255 {
        return Err("名称过长（最多 255 个字符）".into());
    }
    let stem = name.split('.').next().unwrap_or(name).to_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(format!("{} 是 Windows 保留名，不能使用", name));
    }
    Ok(())
}

/// 重命名目录或文件，返回新绝对路径。
/// 不允许重命名根目录、不允许覆盖已存在的目标。
pub fn rename_path(root: &str, path: &str, new_name: &str) -> Result<String, String> {
    let p = ensure_under(root, path)?;
    if !p.exists() {
        return Err(format!("路径不存在：{}", path));
    }
    if canon(&p.to_string_lossy()) == canon(root) {
        return Err("不能重命名根目录".into());
    }
    let name = new_name;
    if name != name.trim() {
        return Err("名称不能以空格开头或结尾".into());
    }
    validate_new_name(name)?;
    let current = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name == current {
        return Ok(p.to_string_lossy().to_string());
    }
    let parent = p.parent().ok_or_else(|| "无法确定上级目录".to_string())?;
    let target = parent.join(name);
    if target.exists() {
        return Err(format!("已存在同名路径：{}", target.display()));
    }
    fs::rename(&p, &target).map_err(|e| format!("重命名失败：{}", e))?;
    // 目录改名 → 同步内部受管说明文件 `<旧名>.txt` → `<新名>.txt`
    //（保证 压缩包名 ≡ 目录名 ≡ 说明文件名 不变量；目标已存在或大小写等价则不动）
    if target.is_dir() {
        let old_note = format!("{}.txt", current);
        let new_note = format!("{}.txt", name);
        if let Ok(rd) = fs::read_dir(&target) {
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                if fname.eq_ignore_ascii_case(&old_note)
                    && e.path().is_file()
                    && !target.join(&new_note).exists()
                {
                    let _ = fs::rename(e.path(), target.join(&new_note));
                    break;
                }
            }
        }
    }
    Ok(target.to_string_lossy().to_string())
}

// ---------------------------------------------------------------- 复制 / 移动

pub fn copy_items(root: &str, items: &[String], dest: &str) -> Result<Vec<String>, String> {
    let dest_dir = ensure_under(root, dest)?;
    // 先校验全部源，避免留下半成品目录
    let mut sources = Vec::with_capacity(items.len());
    for item in items {
        let src = ensure_under(root, item)?;
        if !src.exists() {
            return Err(format!("源不存在：{}", item));
        }
        ensure_not_inside(&dest_dir, &src)?;
        sources.push(src);
    }
    fs::create_dir_all(&dest_dir).map_err(|e| format!("创建目标目录失败：{}", e))?;
    let mut created = Vec::new();
    for src in sources {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| "非法路径".to_string())?;
        let target = unique_path(&dest_dir, &name);
        if src.is_dir() {
            copy_dir(&src, &target)?;
        } else {
            fs::copy(&src, &target).map_err(|e| format!("复制失败：{}", e))?;
        }
        created.push(target.to_string_lossy().to_string());
    }
    Ok(created)
}

pub fn move_items(root: &str, items: &[String], dest: &str) -> Result<Vec<String>, String> {
    let dest_dir = ensure_under(root, dest)?;
    let mut sources = Vec::with_capacity(items.len());
    for item in items {
        let src = ensure_under(root, item)?;
        if !src.exists() {
            return Err(format!("源不存在：{}", item));
        }
        if canon(&dest_dir.to_string_lossy()) == canon(&src.to_string_lossy()) {
            return Err(format!("目标与源相同：{}", item));
        }
        ensure_not_inside(&dest_dir, &src)?;
        sources.push(src);
    }
    fs::create_dir_all(&dest_dir).map_err(|e| format!("创建目标目录失败：{}", e))?;
    let mut moved = Vec::new();
    for src in sources {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| "非法路径".to_string())?;
        let target = unique_path(&dest_dir, &name);
        match fs::rename(&src, &target) {
            Ok(()) => {}
            Err(_) => {
                // 跨盘：复制后删除源
                if src.is_dir() {
                    copy_dir(&src, &target)?;
                    fs::remove_dir_all(&src).map_err(|e| format!("删除源失败：{}", e))?;
                } else {
                    fs::copy(&src, &target).map_err(|e| format!("移动失败：{}", e))?;
                    fs::remove_file(&src).map_err(|e| format!("删除源失败：{}", e))?;
                }
            }
        }
        moved.push(target.to_string_lossy().to_string());
    }
    Ok(moved)
}

/// 目标已存在时追加 ` (2)`、` (3)`…
fn unique_path(dest_dir: &Path, name: &str) -> PathBuf {
    let direct = dest_dir.join(name);
    if !direct.exists() {
        return direct;
    }
    for i in 2..999 {
        let candidate = dest_dir.join(format!("{} ({})", name, i));
        if !candidate.exists() {
            return candidate;
        }
    }
    dest_dir.join(format!("{} ({})", name, std::process::id()))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("创建目录失败：{}", e))?;
    let rd = fs::read_dir(src).map_err(|e| format!("读取源失败：{}", e))?;
    for e in rd.flatten() {
        // 不跟随链接：跳过 symlink/junction，杜绝循环与越界
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if is_reparse(&meta) {
            continue;
        }
        let to = dst.join(e.file_name());
        if meta.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else if meta.is_file() {
            fs::copy(e.path(), &to).map_err(|e| format!("复制失败：{}", e))?;
        }
    }
    Ok(())
}
