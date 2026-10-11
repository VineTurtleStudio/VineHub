use crate::detect::{DetectResult, UnitInfo};
use crate::scan::UnitStats;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const PROJECT_VERSION: u32 = 1;

/// 密码方案（对应密码本「用途 → 密码池」）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordProfile {
    pub id: String,
    pub usage: String,
    pub passwords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveSettings {
    /// rar | zip
    pub format: String,
    /// 分卷大小（MiB），0 = 不分卷
    #[serde(default = "default_volume_mib")]
    pub volume_mib: u64,
    /// 是否把说明文件内容写入压缩注释
    pub comment: bool,
    /// 是否加密文件名（-hp）
    pub encrypt_names: bool,
    /// 覆盖已存在的压缩包
    pub overwrite: bool,
    /// 导出前重新生成说明文件（保证压缩包内含最新说明）
    pub write_notes: bool,
    /// 增量导出：按导出清单指纹跳过源与设置均未变动的单位
    #[serde(default)]
    pub incremental: bool,
}

impl Default for ArchiveSettings {
    fn default() -> Self {
        ArchiveSettings {
            format: "rar".into(),
            volume_mib: default_volume_mib(),
            comment: true,
            encrypt_names: true,
            overwrite: false,
            write_notes: true,
            incremental: false,
        }
    }
}

/// 缺省分卷大小（900 MiB）；显式存 0 表示不分卷，不会被改写
pub fn default_volume_mib() -> u64 {
    900
}

/// 一个最小压缩单位的可编辑条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// 相对根目录的路径（/ 分隔）
    pub rel_path: String,
    /// 目录名（导出时作为压缩包名）
    pub name: String,
    /// album | novel | artbook | other
    pub kind: String,
    pub title: String,
    pub artist: String,
    pub date: String,
    pub label: String,
    pub catalog: String,
    pub publisher: String,
    pub rank: String,
    /// 所属作品（上级作品目录）
    pub work: String,
    /// 分类（直接上级目录名）
    pub category: String,
    pub notes: String,
    pub tags: Vec<String>,
    /// 曲目列表（专辑）
    pub tracklist: Vec<String>,
    pub password_profile: Option<String>,
    pub selected: bool,
    /// pending | done | error
    pub status: String,
    pub has_notes: bool,
    /// 低置信判定，需人工复核；用户手动改类型后清零
    #[serde(default)]
    pub needs_review: bool,
    pub stats: UnitStats,
}

impl Entry {
    pub fn from_unit(unit: &UnitInfo) -> Entry {
        let (work, category) = split_work_and_category(&unit.rel_path);
        Entry {
            rel_path: unit.rel_path.clone(),
            name: unit.name.clone(),
            kind: unit.kind.clone(),
            title: unit.name.clone(),
            artist: String::new(),
            date: String::new(),
            label: String::new(),
            catalog: String::new(),
            publisher: String::new(),
            rank: String::new(),
            work,
            category,
            notes: String::new(),
            tags: Vec::new(),
            tracklist: Vec::new(),
            password_profile: None,
            selected: true,
            status: "pending".into(),
            has_notes: unit.has_notes,
            needs_review: unit.needs_review,
            stats: unit.stats.clone(),
        }
    }

    pub fn parent_rel(&self) -> &str {
        match self.rel_path.rfind('/') {
            Some(i) => &self.rel_path[..i],
            None => "",
        }
    }
}

/// 目录改名同步：就地改写条目 rel_path（含前缀匹配的子孙条目）与 name。
/// 只改路径与目录名，不动用户手工编辑的标题等元数据。
pub fn rename_entry_paths(entries: &mut [Entry], old_rel: &str, new_rel: &str) {
    if old_rel.is_empty() || old_rel == new_rel {
        return;
    }
    let prefix = format!("{}/", old_rel);
    for e in entries.iter_mut() {
        if e.rel_path == old_rel {
            e.rel_path = new_rel.to_string();
            e.name = e
                .rel_path
                .rsplit('/')
                .next()
                .unwrap_or(&e.rel_path)
                .to_string();
        } else if let Some(rest) = e.rel_path.strip_prefix(&prefix) {
            e.rel_path = format!("{}/{}", new_rel, rest);
        }
    }
}

/// 由相对路径推导「所属作品 / 分类」：
/// - 分类 = 直接上级目录名
/// - 所属作品 = 最近的 `NNN ` 数字前缀目录（去掉前缀），没有则取上级目录
pub fn split_work_and_category(rel_path: &str) -> (String, String) {
    let parts: Vec<&str> = rel_path.split('/').collect();
    if parts.len() < 2 {
        return (String::new(), String::new());
    }
    let category = parts[parts.len() - 2].to_string();
    let mut work = String::new();
    for p in parts[..parts.len() - 1].iter().rev() {
        if let Some(stripped) = strip_index_prefix(p) {
            work = stripped;
            break;
        }
    }
    if work.is_empty() {
        work = category.clone();
    }
    (work, category)
}

fn strip_index_prefix(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // 形如 `002 1999 Kanon`：数字 + 空白 才视为序号目录
    if i == 0 || i >= name.len() || !bytes[i].is_ascii_whitespace() {
        return None;
    }
    let rest = name[i..].trim_start();
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// 扫描识别的手动覆盖（合并 / 拆分 / 下钻）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overrides {
    /// 强制为单位（含「并入上级」产生的父路径），停止下钻
    #[serde(default)]
    pub force_units: Vec<String>,
    /// 强制为容器（继续下钻）
    #[serde(default)]
    pub force_containers: Vec<String>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.force_units.is_empty() && self.force_containers.is_empty()
    }

    /// 强制为单位（同时从「强制为容器」移除，保证状态互斥）
    pub fn set_unit(&mut self, rel: &str) {
        self.force_containers.retain(|p| p != rel);
        if !self.force_units.iter().any(|p| p == rel) {
            self.force_units.push(rel.to_string());
        }
    }

    /// 强制为容器
    pub fn set_container(&mut self, rel: &str) {
        self.force_units.retain(|p| p != rel);
        if !self.force_containers.iter().any(|p| p == rel) {
            self.force_containers.push(rel.to_string());
        }
    }

    /// 恢复自动识别：从两组覆盖中移除
    pub fn clear(&mut self, rel: &str) {
        self.force_units.retain(|p| p != rel);
        self.force_containers.retain(|p| p != rel);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub root: String,
    pub output_root: String,
    pub password_profiles: Vec<PasswordProfile>,
    pub archive: ArchiveSettings,
    /// 手动覆盖（旧项目缺省 = 空，行为与自动识别完全一致）
    #[serde(default)]
    pub overrides: Overrides,
    pub entries: Vec<Entry>,
}

impl Project {
    pub fn new(name: &str, root: &str, output_root: &str) -> Project {
        Project {
            version: PROJECT_VERSION,
            name: name.to_string(),
            root: root.to_string(),
            output_root: output_root.to_string(),
            password_profiles: Vec::new(),
            archive: ArchiveSettings::default(),
            overrides: Overrides::default(),
            entries: Vec::new(),
        }
    }

    /// 用扫描结果合并/更新条目（保留已编辑字段）
    pub fn merge_scan(&mut self, result: &DetectResult) {
        let mut index: HashMap<&str, usize> = HashMap::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            index.insert(e.rel_path.as_str(), i);
        }
        let mut next = Vec::with_capacity(result.units.len());
        for unit in &result.units {
            match index.get(unit.rel_path.as_str()) {
                Some(&i) => {
                    let mut e = self.entries[i].clone();
                    e.stats = unit.stats.clone();
                    e.has_notes = unit.has_notes;
                    e.name = unit.name.clone();
                    next.push(e);
                }
                None => next.push(Entry::from_unit(unit)),
            }
        }
        self.entries = next;
    }

    /// 为条目解析实际使用的密码
    pub fn resolve_password(&self, entry: &Entry) -> Option<String> {
        resolve_password(&self.password_profiles, entry)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        // 原子写：tmp → （旧文件先留 .bak）→ rename 覆盖
        let tmp = sidecar(path, "tmp");
        write_sync(&tmp, json.as_bytes())?;
        if path.exists() {
            let bak = sidecar(path, "bak");
            if let Err(e) = fs::copy(path, &bak) {
                let _ = fs::remove_file(&tmp);
                return Err(format!("备份失败: {}", e));
            }
        }
        if let Err(e) = fs::rename(&tmp, path) {
            return Err(format!("保存失败: {}", e));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Project, String> {
        let primary = load_one(path);
        match primary {
            Ok(p) => Ok(p),
            Err(main_err) => {
                let bak = sidecar(path, "bak");
                if bak.exists() {
                    if let Ok(p) = load_one(&bak) {
                        eprintln!("[project] 主文件读取失败（{}），已回退 .bak", main_err);
                        return Ok(p);
                    }
                }
                Err(main_err)
            }
        }
    }
}

/// `<path>.<ext>` 形式的旁路文件（保留原扩展名）
fn sidecar(path: &Path, ext: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".");
    os.push(ext);
    PathBuf::from(os)
}

fn write_sync(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = fs::File::create(path).map_err(|e| format!("写入失败: {}", e))?;
    f.write_all(bytes).map_err(|e| format!("写入失败: {}", e))?;
    f.sync_all().map_err(|e| format!("落盘失败: {}", e))?;
    Ok(())
}

fn load_one(path: &Path) -> Result<Project, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取失败: {}", e))?;
    let p: Project = serde_json::from_str(&text).map_err(|e| format!("解析失败: {}", e))?;
    if p.version > PROJECT_VERSION {
        return Err(format!(
            "项目版本 {} 高于当前支持的 {}",
            p.version, PROJECT_VERSION
        ));
    }
    Ok(p)
}

fn stable_hash(s: &str) -> u64 {
    // FNV-1a
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 按密码方案解析条目实际使用的密码（多密码时稳定哈希选择）
pub fn resolve_password(profiles: &[PasswordProfile], entry: &Entry) -> Option<String> {
    let profile_id = entry.password_profile.as_ref()?;
    let profile = profiles.iter().find(|p| &p.id == profile_id)?;
    if profile.passwords.is_empty() {
        return None;
    }
    if profile.passwords.len() == 1 {
        return Some(profile.passwords[0].clone());
    }
    let idx = stable_hash(&entry.rel_path) as usize % profile.passwords.len();
    Some(profile.passwords[idx].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_and_category() {
        assert_eq!(
            split_work_and_category("Key小说图册/Key 官方作品/002 1999 Kanon（カノン）/画集/Kanon Fanbook"),
            ("1999 Kanon（カノン）".to_string(), "画集".to_string())
        );
        assert_eq!(
            split_work_and_category("Key小说图册/Key 官方作品/000 画师作品集/ZEN/HAPPY TURN EX"),
            ("画师作品集".to_string(), "ZEN".to_string())
        );
        assert_eq!(
            split_work_and_category("KEY音乐/01_Key Sounds Label/KSLA/[2015.09.30] xxx"),
            ("KSLA".to_string(), "KSLA".to_string())
        );
        assert_eq!(
            split_work_and_category(
                "Key小说图册/Key 官方作品/000 画师作品集/ZEN/HAPPY TURN EX"
            ),
            ("画师作品集".to_string(), "ZEN".to_string())
        );
    }

    #[test]
    fn password_pool_is_stable() {
        let mut p = Project::new("t", "a", "b");
        p.password_profiles.push(PasswordProfile {
            id: "books".into(),
            usage: "小说图册等".into(),
            passwords: vec!["A".into(), "B".into(), "C".into()],
        });
        let mut e = Entry::from_unit(&UnitInfo {
            rel_path: "x/y".into(),
            name: "y".into(),
            kind: "artbook".into(),
            has_notes: false,
            needs_review: false,
            stats: UnitStats::default(),
        });
        e.password_profile = Some("books".into());
        let first = p.resolve_password(&e);
        assert!(first.is_some());
        assert_eq!(first, p.resolve_password(&e));
    }
}
