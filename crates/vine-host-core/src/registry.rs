//! 插件注册表：扫描目录 → 解析校验 → 缓存 → 状态跟踪。
//!
//! 单个插件解析失败只影响该插件（标记 `Unavailable` 并附原因），不影响其他插件。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vine_plugin_api::{ManifestError, PluginKind, PluginManifest, PLUGIN_MANIFEST_FILENAME};

/// 注册表 I/O 错误。
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("插件目录不可读: {0}")]
    Io(#[from] std::io::Error),
}

/// 插件可用状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// 清单与入口均校验通过。
    Available,
    /// 不可用，附带用户可读原因。
    Unavailable { reason: String },
}

impl PluginStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// 注册表中的一个插件条目。
#[derive(Debug, Clone)]
pub struct PluginEntry {
    /// 清单（解析失败时为 None，仅保留目录名做展示键）。
    pub manifest: Option<PluginManifest>,
    /// 插件根目录。
    pub root: PathBuf,
    /// `plugin.json` 路径。
    pub manifest_path: PathBuf,
    /// 展示键：解析成功为 `manifest.id`，否则为目录名。
    pub key: String,
    pub status: PluginStatus,
}

impl PluginEntry {
    pub fn id(&self) -> &str {
        &self.key
    }

    pub fn kind(&self) -> Option<PluginKind> {
        self.manifest.as_ref().map(|m| m.kind)
    }

    pub fn display_name(&self) -> &str {
        match &self.manifest {
            Some(m) => &m.name,
            None => &self.key,
        }
    }
}

/// 一次扫描的结果摘要。
#[derive(Debug, Default, Clone)]
pub struct ScanSummary {
    /// 成功载入（Available）的插件键。
    pub loaded: Vec<String>,
    /// 失败（Unavailable）的 (键, 原因)。
    pub failed: Vec<(String, String)>,
}

/// 插件注册表。
#[derive(Debug)]
pub struct PluginRegistry {
    plugins_dir: PathBuf,
    entries: BTreeMap<String, PluginEntry>,
}

impl PluginRegistry {
    pub fn new(plugins_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins_dir: plugins_dir.into(),
            entries: BTreeMap::new(),
        }
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// 全量重扫（即热重载）：以磁盘现状重建缓存。
    pub fn scan(&mut self) -> Result<ScanSummary, RegistryError> {
        let mut entries: BTreeMap<String, PluginEntry> = BTreeMap::new();
        let mut conflicts: Vec<(String, String)> = Vec::new();

        if self.plugins_dir.is_dir() {
            let mut dirs: Vec<PathBuf> = Vec::new();
            for item in std::fs::read_dir(&self.plugins_dir)? {
                let item = item?;
                if item.file_type()?.is_dir() {
                    dirs.push(item.path());
                }
            }
            dirs.sort();
            for root in dirs {
                let entry = Self::load_dir(&root);
                // 键冲突：保留先加载者（目录名排序稳定），后来者记入失败列表
                if let Some(existing) = entries.get(&entry.key) {
                    conflicts.push((
                        entry.key.clone(),
                        format!(
                            "插件 id 冲突：{} 已被 {} 占用",
                            entry.key,
                            existing.root.display()
                        ),
                    ));
                } else {
                    entries.insert(entry.key.clone(), entry);
                }
            }
        }

        let mut summary = ScanSummary {
            failed: conflicts,
            ..Default::default()
        };
        for (key, entry) in &entries {
            match &entry.status {
                PluginStatus::Available => summary.loaded.push(key.clone()),
                PluginStatus::Unavailable { reason } => {
                    summary.failed.push((key.clone(), reason.clone()))
                }
            }
        }
        self.entries = entries;
        Ok(summary)
    }

    fn load_dir(root: &Path) -> PluginEntry {
        let manifest_path = root.join(PLUGIN_MANIFEST_FILENAME);
        let dir_key = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());

        let json = match std::fs::read_to_string(&manifest_path) {
            Ok(json) => json,
            Err(e) => {
                return PluginEntry {
                    manifest: None,
                    root: root.to_path_buf(),
                    manifest_path,
                    key: dir_key,
                    status: PluginStatus::Unavailable {
                        reason: format!("无法读取 {PLUGIN_MANIFEST_FILENAME}: {e}"),
                    },
                };
            }
        };

        match PluginManifest::from_json(&json) {
            Ok(manifest) => {
                let status = match manifest.validate_with_root(root) {
                    Ok(_) => PluginStatus::Available,
                    Err(ManifestError::EntryNotFound(path)) => PluginStatus::Unavailable {
                        // 入口缺失在 P0 external 场景可由设置页路径覆盖补齐，仍按规范标不可用
                        reason: format!("入口文件不存在: {path}（可在设置页配置覆盖路径）"),
                    },
                    Err(e) => PluginStatus::Unavailable {
                        reason: e.to_string(),
                    },
                };
                let key = manifest.id.clone();
                PluginEntry {
                    manifest: Some(manifest),
                    root: root.to_path_buf(),
                    manifest_path,
                    key,
                    status,
                }
            }
            Err(e) => PluginEntry {
                manifest: None,
                root: root.to_path_buf(),
                manifest_path,
                key: dir_key,
                status: PluginStatus::Unavailable {
                    reason: format!("manifest 校验失败: {e}"),
                },
            },
        }
    }

    pub fn get(&self, id: &str) -> Option<&PluginEntry> {
        self.entries.get(id)
    }

    pub fn list(&self) -> impl Iterator<Item = &PluginEntry> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 移除单个插件（卸载/禁用后重扫前的显式操作）。
    pub fn remove(&mut self, id: &str) -> bool {
        self.entries.remove(id).is_some()
    }

    /// 入口解析顺序：绝对路径 > 插件目录相对 > 设置页覆盖 > PATH。
    ///
    /// 返回第一个真实存在的候选；均不存在时返回「覆盖路径或插件目录相对」的首选候选
    /// （由调用方决定报错展示）。
    pub fn resolve_entry(&self, id: &str, settings_override: Option<&Path>) -> Option<PathBuf> {
        let entry = self.entries.get(id)?;
        let manifest = entry.manifest.as_ref()?;
        let raw = manifest.entry.as_str();
        let raw_path = Path::new(raw);

        let mut candidates: Vec<PathBuf> = Vec::new();
        if raw_path.is_absolute() {
            candidates.push(raw_path.to_path_buf());
        } else {
            candidates.push(entry.root.join(raw_path));
        }
        if let Some(over) = settings_override {
            if !over.as_os_str().is_empty() {
                candidates.push(over.to_path_buf());
            }
        }
        candidates.into_iter().find(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_manifest(dir: &Path, id: &str, entry: &str, version: &str) {
        fs::create_dir_all(dir).unwrap();
        let json = format!(
            r#"{{
                "schemaVersion": 1,
                "id": "{id}",
                "name": "{id}",
                "version": "{version}",
                "apiVersion": "1.0",
                "kind": "external",
                "entry": "{entry}",
                "panels": [],
                "commands": [],
                "permissions": {{ "fs.read": [], "fs.write": [], "net": false, "exec": false }},
                "dataDir": "{id}",
                "minHubVersion": "0.1.0"
            }}"#
        );
        fs::write(dir.join("plugin.json"), json).unwrap();
    }

    #[test]
    fn loads_valid_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("pulse");
        let p2 = tmp.path().join("press");
        write_manifest(&p1, "vine.pulse", "bin/app.exe", "0.1.0");
        write_manifest(&p2, "vine.press", "bin/app.exe", "0.2.0");
        fs::create_dir_all(p1.join("bin")).unwrap();
        fs::write(p1.join("bin/app.exe"), b"x").unwrap();
        fs::create_dir_all(p2.join("bin")).unwrap();
        fs::write(p2.join("bin/app.exe"), b"x").unwrap();

        let mut reg = PluginRegistry::new(tmp.path());
        let summary = reg.scan().unwrap();
        assert_eq!(summary.loaded, vec!["vine.press", "vine.pulse"]);
        assert!(summary.failed.is_empty());
        assert_eq!(reg.get("vine.pulse").unwrap().status, PluginStatus::Available);
        assert_eq!(
            reg.get("vine.pulse").unwrap().kind(),
            Some(PluginKind::External)
        );
    }

    #[test]
    fn single_plugin_failure_is_isolated() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("good");
        let bad_json = tmp.path().join("bad-json");
        let missing_entry = tmp.path().join("missing-entry");

        write_manifest(&good, "vine.good", "bin/app.exe", "0.1.0");
        fs::create_dir_all(good.join("bin")).unwrap();
        fs::write(good.join("bin/app.exe"), b"x").unwrap();

        fs::create_dir_all(&bad_json).unwrap();
        fs::write(bad_json.join("plugin.json"), "{ not json").unwrap();

        write_manifest(&missing_entry, "vine.missing", "bin/app.exe", "0.1.0");

        let mut reg = PluginRegistry::new(tmp.path());
        let summary = reg.scan().unwrap();
        assert_eq!(summary.loaded, vec!["vine.good"]);
        assert_eq!(summary.failed.len(), 2);

        // 坏插件被标记 Unavailable 且带原因，好插件不受影响
        let bad = reg.get("bad-json").unwrap();
        assert!(!bad.status.is_available());
        match &bad.status {
            PluginStatus::Unavailable { reason } => assert!(reason.contains("manifest")),
            _ => panic!("应为 Unavailable"),
        }
        let missing = reg.get("vine.missing").unwrap();
        match &missing.status {
            PluginStatus::Unavailable { reason } => assert!(reason.contains("入口文件不存在")),
            _ => panic!("应为 Unavailable"),
        }
        assert!(reg.get("vine.good").unwrap().status.is_available());
    }

    #[test]
    fn hot_reload_reflects_disk_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pulse");
        write_manifest(&p, "vine.pulse", "bin/app.exe", "0.1.0");
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::write(p.join("bin/app.exe"), b"x").unwrap();

        let mut reg = PluginRegistry::new(tmp.path());
        reg.scan().unwrap();
        assert_eq!(
            reg.get("vine.pulse").unwrap().manifest.as_ref().unwrap().version,
            "0.1.0"
        );

        // 磁盘上改版本 + 新增插件，热重载后均生效
        write_manifest(&p, "vine.pulse", "bin/app.exe", "0.9.0");
        let p2 = tmp.path().join("sync");
        write_manifest(&p2, "vine.sync", "bin/app.exe", "0.1.0");
        fs::create_dir_all(p2.join("bin")).unwrap();
        fs::write(p2.join("bin/app.exe"), b"x").unwrap();

        let summary = reg.scan().unwrap();
        assert_eq!(summary.loaded, vec!["vine.pulse", "vine.sync"]);
        assert_eq!(
            reg.get("vine.pulse").unwrap().manifest.as_ref().unwrap().version,
            "0.9.0"
        );
    }

    #[test]
    fn resolve_entry_prefers_existing_and_honors_override() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pulse");
        write_manifest(&p, "vine.pulse", "bin/app.exe", "0.1.0");
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::write(p.join("bin/app.exe"), b"x").unwrap();

        let mut reg = PluginRegistry::new(tmp.path());
        reg.scan().unwrap();

        // 插件目录相对命中
        assert_eq!(
            reg.resolve_entry("vine.pulse", None).unwrap(),
            p.join("bin/app.exe")
        );

        // 入口缺失时回退到设置页覆盖
        fs::remove_file(p.join("bin/app.exe")).unwrap();
        assert!(reg.resolve_entry("vine.pulse", None).is_none());
        let override_exe = tmp.path().join("elsewhere.exe");
        fs::write(&override_exe, b"x").unwrap();
        assert_eq!(
            reg.resolve_entry("vine.pulse", Some(&override_exe)).unwrap(),
            override_exe
        );
    }
}
