use crate::model::Config;
use std::path::Path;

/// 读取配置；文件不存在或损坏时返回默认配置（不覆盖原文件）
pub fn load(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// 原子写入配置（先写临时文件再重命名）
pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::msg!("创建配置目录失败: {e}", "Failed to create config directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| crate::msg!("配置序列化失败: {e}", "Config serialization failed: {e}"))?;
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_name);
    std::fs::write(&tmp, json).map_err(|e| crate::msg!("写入配置失败: {e}", "Failed to write config: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| crate::msg!("保存配置失败: {e}", "Failed to save config: {e}"))?;
    Ok(())
}
