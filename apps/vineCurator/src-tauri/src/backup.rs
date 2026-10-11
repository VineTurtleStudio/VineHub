//! 备份（纯逻辑，零 Tauri）：元数据快照 + restic 集成钩子。
//! 快照 = `VACUUM INTO` 对 catalog.sqlite 的在线一致副本（含 FTS 索引），落
//! `<backup_dir>/catalog-<epoch_ms>.sqlite`，按 `backup_keep` 保留最新 N 份；
//! `restic_repo` 非空时执行 `restic backup <快照目录>`，缺失/失败降级不崩溃
//! （本地快照始终保留）。媒体文件与原始数据不参与——快照只碰数据库与 backups 目录。

use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::repo::{self, SettingEntry};
use crate::thumbs;

pub trait ResticRunner: Send {
    fn backup(&self, dir: &Path, repo_url: &str, password_file: &str) -> Result<String, String>;
}

pub struct CliRestic {
    pub program: String,
}

impl CliRestic {
    pub fn from_overrides(overrides: &HashMap<String, String>) -> Self {
        let configured = overrides
            .get("restic_path")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        Self {
            program: configured.unwrap_or("restic").to_string(),
        }
    }
}

impl ResticRunner for CliRestic {
    fn backup(&self, dir: &Path, repo_url: &str, password_file: &str) -> Result<String, String> {
        let mut cmd = Command::new(&self.program);
        cmd.arg("--quiet")
            .arg("backup")
            .arg(dir)
            .arg("--repo")
            .arg(repo_url);
        if !password_file.trim().is_empty() {
            cmd.arg("--password-file").arg(password_file.trim());
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let out = cmd
            .output()
            .map_err(|e| format!("启动 restic 失败（检查 restic_path / PATH）: {e}"))?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("restic ok")
                .trim()
                .to_string())
        } else {
            let tail: String = String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(400)
                .collect();
            Err(format!("restic 退出 {:?}: {tail}", out.status.code()))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupReport {
    pub snapshot_path: String,
    pub kept: usize,
    /// restic 成功时的摘要行
    pub restic: Option<String>,
    /// 未配置 / 失败时的降级原因（本地快照不受影响）
    pub restic_skipped: Option<String>,
}

pub struct BackupConfig {
    pub dir: String,
    pub keep: usize,
    pub restic_repo: String,
    pub restic_password_file: String,
}

pub fn load_config(conn: &Connection) -> BackupConfig {
    let get = |k: &str| repo::get_setting_value(conn, k).unwrap_or_default();
    let keep: usize = get("backup_keep").trim().parse().unwrap_or(7);
    BackupConfig {
        dir: get("backup_dir"),
        keep: keep.max(1),
        restic_repo: get("restic_repo"),
        restic_password_file: get("restic_password_file"),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 保留快照文件列表（时间序：ms 字典序 + 同 ms 内 base < -1 < -2）
fn snapshot_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    name.starts_with("catalog-") && name.ends_with(".sqlite")
                })
                .collect()
        })
        .unwrap_or_default();
    fn key(p: &Path) -> (String, u32) {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        match stem
            .strip_prefix("catalog-")
            .and_then(|s| s.split_once('-'))
        {
            Some((ms, n)) => (ms.to_string(), n.parse().unwrap_or(0)),
            None => (stem.to_string(), 0),
        }
    }
    files.sort_by_key(|p| key(p));
    files
}

/// VACUUM INTO 在线一致快照 + 按 keep 清理最旧；返回快照绝对路径
pub fn snapshot(conn: &Connection, dir: &Path, keep: usize) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    let ms = now_ms();
    let mut path = dir.join(format!("catalog-{ms}.sqlite"));
    let mut n = 1u32;
    while path.exists() {
        path = dir.join(format!("catalog-{ms}-{n}.sqlite"));
        n += 1;
    }
    let escaped = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
        .map_err(|e| format!("快照失败: {e}"))?;
    prune(dir, keep)?;
    Ok(path)
}

/// 只保留最新 keep 份（keep 钳 ≥1）；返回保留数。单个删除失败不阻断。
pub fn prune(dir: &Path, keep: usize) -> Result<usize, String> {
    let files = snapshot_files(dir);
    let keep = keep.max(1);
    if files.len() > keep {
        for old in files.iter().take(files.len() - keep) {
            let _ = fs::remove_file(old);
        }
    }
    Ok(files.len().min(keep))
}

/// 一次备份：快照 + （可选）restic 推送 + 记 `last_backup_at`（epoch ms）
pub fn run_backup(
    conn: &mut Connection,
    default_derived: &Path,
    restic: Option<&dyn ResticRunner>,
) -> Result<BackupReport, String> {
    let cfg = load_config(conn);
    let dir = if cfg.dir.trim().is_empty() {
        thumbs::derived_base(conn, default_derived)?.join("backups")
    } else {
        PathBuf::from(cfg.dir.trim())
    };
    let path = snapshot(conn, &dir, cfg.keep)?;
    let kept = snapshot_files(&dir).len();

    let (restic_summary, skipped) = if cfg.restic_repo.trim().is_empty() {
        (None, Some("未配置 restic_repo（本地快照已完成）".into()))
    } else if let Some(runner) = restic {
        match runner.backup(
            &dir,
            cfg.restic_repo.trim(),
            cfg.restic_password_file.trim(),
        ) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(format!("restic 失败（本地快照已完成）: {e}"))),
        }
    } else {
        (None, Some("未提供 restic 执行器".into()))
    };

    repo::save_settings(
        conn,
        &[SettingEntry {
            key: "last_backup_at".into(),
            value: now_ms().to_string(),
        }],
    )
    .map_err(|e| format!("记录备份时间失败: {e}"))?;

    Ok(BackupReport {
        snapshot_path: path.to_string_lossy().into_owned(),
        kept,
        restic: restic_summary,
        restic_skipped: skipped,
    })
}
