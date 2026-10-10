//! `plugin.json` 清单结构与校验。
//!
//! 校验规则（规范 v1）：
//! - id 格式 `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`，全局唯一
//! - 版本号 / apiVersion / minHubVersion 为合法 `major.minor[.patch]`
//! - apiVersion 与宿主兼容（major 相同且 minor 不高于宿主）
//! - 相对路径不得越出插件目录（禁止 `..`、绝对路径、盘符前缀）
//! - 入口文件存在性（需提供插件根目录时检查）

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::version::{ApiVersion, VersionError};

/// 校验错误。
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("不支持的 schemaVersion: {0}（当前宿主仅支持 1）")]
    UnsupportedSchemaVersion(u64),

    #[error("插件 id 非法: {0}")]
    InvalidId(String),

    #[error("字段 {0} 不能为空")]
    EmptyField(&'static str),

    #[error("版本号非法: {0}")]
    InvalidVersion(String),

    #[error("apiVersion 非法: {0}")]
    InvalidApiVersion(String),

    #[error("apiVersion 不兼容: 插件 {plugin} vs 宿主 {host}")]
    ApiVersionIncompatible { plugin: String, host: String },

    #[error("路径越界或非法: {0}")]
    PathEscape(String),

    #[error("入口文件不存在: {0}")]
    EntryNotFound(String),
}

/// 插件生命周期等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// 仅负责一键启动独立程序（P0 形态）。
    External,
    /// 宿主拉起子进程走 JSON-RPC（P1+ 形态）。
    Sidecar,
}

/// 面板声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelSpec {
    pub id: String,
    pub title: String,
    /// 插件目录内的前端入口（如 `dist/index.html`）。
    pub ui: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// 命令声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
}

/// 文件访问模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FsMode {
    /// 仅用户当场选择的路径。
    UserPicked,
    /// 用户已授权的根路径集合。
    AllSelected,
}

/// 权限声明（信任模型：UI 明示 + 宿主转发闸门，非 OS 沙箱）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(rename = "fs.read", default)]
    pub fs_read: Vec<FsMode>,
    #[serde(rename = "fs.write", default)]
    pub fs_write: Vec<FsMode>,
    #[serde(default)]
    pub net: bool,
    #[serde(default)]
    pub exec: bool,
}

/// 完整插件清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub schema_version: u64,
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub kind: PluginKind,
    pub entry: String,
    #[serde(default)]
    pub panels: Vec<PanelSpec>,
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    #[serde(default)]
    pub permissions: Permissions,
    pub data_dir: String,
    pub min_hub_version: String,
}

impl PluginManifest {
    /// 解析 JSON 并做不依赖文件系统的结构校验。
    pub fn from_json(json: &str) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_str(json)?;
        manifest.validate_structure()?;
        Ok(manifest)
    }

    /// 结构校验（不检查文件存在性）。
    pub fn validate_structure(&self) -> Result<(), ManifestError> {
        if self.schema_version != 1 {
            return Err(ManifestError::UnsupportedSchemaVersion(self.schema_version));
        }
        if !is_valid_plugin_id(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        require_non_empty(&self.name, "name")?;
        require_non_empty(&self.version, "version")?;
        require_non_empty(&self.data_dir, "dataDir")?;
        require_semver(&self.version, "version")?;
        require_semver(&self.min_hub_version, "minHubVersion")?;

        let plugin_api: ApiVersion = self
            .api_version
            .parse()
            .map_err(|_: VersionError| ManifestError::InvalidApiVersion(self.api_version.clone()))?;
        let host_api: ApiVersion = crate::HOST_API_VERSION
            .parse()
            .expect("HOST_API_VERSION 是常量且合法");
        if !plugin_api.is_compatible_with_host(host_api) {
            return Err(ManifestError::ApiVersionIncompatible {
                plugin: self.api_version.clone(),
                host: crate::HOST_API_VERSION.to_string(),
            });
        }

        require_non_empty(&self.entry, "entry")?;
        if is_relative(&self.entry) && !is_safe_plugin_relative_path(&self.entry) {
            return Err(ManifestError::PathEscape(self.entry.clone()));
        }
        if !is_safe_data_dir(&self.data_dir) {
            return Err(ManifestError::PathEscape(self.data_dir.clone()));
        }

        for panel in &self.panels {
            require_non_empty(&panel.id, "panels[].id")?;
            require_non_empty(&panel.title, "panels[].title")?;
            if !is_safe_plugin_relative_path(&panel.ui) {
                return Err(ManifestError::PathEscape(panel.ui.clone()));
            }
        }
        for command in &self.commands {
            require_non_empty(&command.id, "commands[].id")?;
            require_non_empty(&command.title, "commands[].title")?;
        }
        Ok(())
    }

    /// 结构校验 + 以 `root`（插件目录）为基准确认入口文件存在。
    pub fn validate_with_root(&self, root: &Path) -> Result<PathBuf, ManifestError> {
        self.validate_structure()?;
        let entry = resolve_entry_path(&self.entry, root);
        if !entry.is_file() {
            return Err(ManifestError::EntryNotFound(entry.display().to_string()));
        }
        Ok(entry)
    }
}

/// 入口解析顺序（规范 v1）：绝对路径 > 插件目录相对 > 设置页覆盖 > PATH。
/// 此处实现前两级；设置页覆盖与 PATH 由宿主层处理。
pub fn resolve_entry_path(entry: &str, root: &Path) -> PathBuf {
    let path = Path::new(entry);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// 插件 id 格式：`^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`。
pub fn is_valid_plugin_id(id: &str) -> bool {
    let mut segments = id.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let mut chars = first.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return false;
    }
    segments.all(|seg| {
        !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// 相对路径安全性：非绝对、无盘符前缀、无 `..`、无根目录组件。
pub fn is_safe_plugin_relative_path(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let path = Path::new(p);
    if path.is_absolute() {
        return false;
    }
    !path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

fn is_relative(p: &str) -> bool {
    !Path::new(p).is_absolute()
}

/// dataDir 作为数据目录名：禁止路径分隔符、`..`、盘符。
fn is_safe_data_dir(p: &str) -> bool {
    !p.is_empty()
        && !p.contains(['/', '\\', ':'])
        && p != "."
        && p != ".."
        && !p.contains("..")
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), ManifestError> {
    if value.trim().is_empty() {
        return Err(ManifestError::EmptyField(field));
    }
    Ok(())
}

/// 轻量 semver 校验：`major.minor.patch`，允许 `-` 后缀（如 `1.0.0-beta.1`）。
fn require_semver(value: &str, field: &'static str) -> Result<(), ManifestError> {
    let core = value.split('-').next().unwrap_or("");
    let mut parts = core.split('.');
    let mut count = 0;
    for part in parts.by_ref() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ManifestError::InvalidVersion(format!("{value}（字段 {field}）")));
        }
        count += 1;
    }
    if count != 3 {
        return Err(ManifestError::InvalidVersion(format!("{value}（字段 {field}）")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn valid_manifest_json(id: &str, kind: &str) -> String {
        format!(
            r#"{{
                "schemaVersion": 1,
                "id": "{id}",
                "name": "TestPlugin",
                "version": "0.1.0",
                "apiVersion": "1.0",
                "kind": "{kind}",
                "entry": "bin/test.exe",
                "panels": [{{ "id": "main", "title": "主面板", "ui": "dist/index.html", "icon": "pulse" }}],
                "commands": [{{ "id": "run.start", "title": "执行" }}],
                "permissions": {{ "fs.read": ["user-picked"], "fs.write": [], "net": false, "exec": false }},
                "dataDir": "{id}",
                "minHubVersion": "0.1.0"
            }}"#
        )
    }

    #[test]
    fn five_first_party_manifests_are_valid() {
        for id in [
            "vine.pulse",
            "vine.press",
            "vine.sync",
            "vine.curator",
            "vine.cursor",
        ] {
            let m = PluginManifest::from_json(&valid_manifest_json(id, "external"))
                .unwrap_or_else(|e| panic!("{id} 应当合法: {e}"));
            assert_eq!(m.id, id);
        }
    }

    #[test]
    fn sidecar_kind_parses() {
        let m = PluginManifest::from_json(&valid_manifest_json("vine.pulse", "sidecar")).unwrap();
        assert_eq!(m.kind, PluginKind::Sidecar);
    }

    #[test]
    fn reject_bad_ids() {
        for bad in [
            "Vine.Pulse",
            "1pulse",
            "pulse..x",
            "pulse.x-y",
            "pulse.",
            ".pulse",
            "",
            "PULSE",
        ] {
            let json = valid_manifest_json(bad, "external");
            let err = PluginManifest::from_json(&json).unwrap_err();
            assert!(
                matches!(err, ManifestError::InvalidId(_) | ManifestError::Json(_)),
                "id {bad:?} 应被拒绝，得到 {err:?}"
            );
        }
    }

    #[test]
    fn reject_bad_versions() {
        let json = valid_manifest_json("vine.pulse", "external")
            .replace(r#""version": "0.1.0""#, r#""version": "abc""#);
        assert!(matches!(
            PluginManifest::from_json(&json).unwrap_err(),
            ManifestError::InvalidVersion(_)
        ));

        let json = valid_manifest_json("vine.pulse", "external")
            .replace(r#""minHubVersion": "0.1.0""#, r#""minHubVersion": "nope""#);
        assert!(matches!(
            PluginManifest::from_json(&json).unwrap_err(),
            ManifestError::InvalidVersion(_)
        ));
    }

    #[test]
    fn reject_incompatible_api_version() {
        for api in ["2.0", "1.5", "0.9"] {
            let json = valid_manifest_json("vine.pulse", "external")
                .replace(r#""apiVersion": "1.0""#, &format!(r#""apiVersion": "{api}""#));
            let err = PluginManifest::from_json(&json).unwrap_err();
            assert!(
                matches!(
                    err,
                    ManifestError::ApiVersionIncompatible { .. } | ManifestError::InvalidApiVersion(_)
                ),
                "apiVersion {api} 应被拒绝，得到 {err:?}"
            );
        }
    }

    #[test]
    fn reject_path_escape() {
        for bad_ui in [
            "../evil.html",
            "a/../../b.html",
            "/abs/index.html",
            "C:\\\\evil.html",
        ] {
            let json = valid_manifest_json("vine.pulse", "external")
                .replace("dist/index.html", bad_ui);
            let err = PluginManifest::from_json(&json).unwrap_err();
            assert!(
                matches!(err, ManifestError::PathEscape(_)),
                "ui {bad_ui:?} 应被拒绝，得到 {err:?}"
            );
        }

        let json = valid_manifest_json("vine.pulse", "external")
            .replace("bin/test.exe", "../../evil.exe");
        assert!(matches!(
            PluginManifest::from_json(&json).unwrap_err(),
            ManifestError::PathEscape(_)
        ));
    }

    #[test]
    fn reject_bad_schema_version_and_kind() {
        let json = valid_manifest_json("vine.pulse", "external")
            .replace(r#""schemaVersion": 1"#, r#""schemaVersion": 99"#);
        assert!(matches!(
            PluginManifest::from_json(&json).unwrap_err(),
            ManifestError::UnsupportedSchemaVersion(99)
        ));

        let json = valid_manifest_json("vine.pulse", "dylib");
        assert!(PluginManifest::from_json(&json).is_err());
    }

    #[test]
    fn reject_bad_data_dir() {
        for bad in ["a/b", "..", "a\\\\b", "C:x"] {
            let json = valid_manifest_json("vine.pulse", "external")
                .replace(r#""dataDir": "vine.pulse""#, &format!(r#""dataDir": "{bad}""#));
            let err = PluginManifest::from_json(&json).unwrap_err();
            assert!(
                matches!(err, ManifestError::PathEscape(_)),
                "dataDir {bad:?} 应被拒绝，得到 {err:?}"
            );
        }
    }

    #[test]
    fn absolute_entry_is_allowed_structure_only() {
        let json = valid_manifest_json("vine.pulse", "external")
            .replace("bin/test.exe", "D:/tools/vine-pulse.exe");
        assert!(PluginManifest::from_json(&json).is_ok());
    }

    #[test]
    fn validate_with_root_checks_entry_existence() {
        let dir = std::env::temp_dir().join("vine-plugin-api-test-root");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/test.exe"), b"x").unwrap();

        let m = PluginManifest::from_json(&valid_manifest_json("vine.pulse", "external")).unwrap();
        assert!(m.validate_with_root(&dir).is_ok());

        std::fs::remove_file(dir.join("bin/test.exe")).unwrap();
        assert!(matches!(
            m.validate_with_root(&dir).unwrap_err(),
            ManifestError::EntryNotFound(_)
        ));
    }
}
