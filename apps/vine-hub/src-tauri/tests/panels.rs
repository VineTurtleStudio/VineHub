//! P3.1 面板解析回归：manifest panels 可解析且 ui 文件真实存在。

use std::path::PathBuf;

use vine_host_core::PluginRegistry;

fn repo_plugins_dir() -> PathBuf {
    // apps/vine-hub/src-tauri → 仓库根
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins")
}

#[test]
fn all_first_party_panels_resolve_to_existing_files() {
    let plugins_dir = repo_plugins_dir();
    assert!(plugins_dir.is_dir(), "plugins dir: {}", plugins_dir.display());

    let mut registry = PluginRegistry::new(&plugins_dir);
    registry.scan().expect("扫描插件目录");

    // P3 迁移目标（vinePulse 为原生面板，无 dist 要求）
    let expected = ["vine.press", "vine.sync", "vine.curator", "vine.cursor"];
    for id in expected {
        let entry = registry.get(id).unwrap_or_else(|| panic!("插件 {id} 未加载"));
        let manifest = entry.manifest.as_ref().unwrap_or_else(|| {
            panic!("{id} 无 manifest，status={:?}", entry.status)
        });
        let panel = manifest
            .panels
            .iter()
            .find(|p| p.id == "main")
            .unwrap_or_else(|| panic!("{id} 缺少 main 面板"));
        assert_eq!(panel.ui, "dist/index.html");
        let ui_path = entry.root.join(&panel.ui);
        assert!(
            ui_path.is_file(),
            "{id} 面板入口不存在: {}（请先构建并拷贝 dist）",
            ui_path.display()
        );
    }
}

#[test]
fn pulse_manifest_panel_is_declared_but_not_required_on_disk() {
    let plugins_dir = repo_plugins_dir();
    let mut registry = PluginRegistry::new(&plugins_dir);
    registry.scan().expect("扫描插件目录");
    let entry = registry.get("vine.pulse").expect("vine.pulse 未加载");
    let manifest = entry
        .manifest
        .as_ref()
        .unwrap_or_else(|| panic!("vine.pulse 无 manifest，status={:?}", entry.status));
    // 声明保留（历史），P3 不迁移 pulse；文件缺失不阻断
    assert!(!manifest.panels.is_empty());
}
