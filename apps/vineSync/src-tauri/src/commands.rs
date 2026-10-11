use crate::core::filter::Filter;
use crate::core::{drive_available, normalize_path_str, version};
use crate::model::{AppInfoDto, CleanupRequest, Config, FilterTestDto, PreviewRequest, PreviewDto, StateDto, SyncRequest, LogRow};
use crate::runtime;
use crate::storage;
use crate::trigger;
use crate::AppCtx;
use std::path::Path;
use tauri::Manager;

pub(crate) fn normalize_pair(pair: &mut crate::model::PairConfig) {
    pair.name = pair.name.trim().to_string();
    pair.source = pair.source.trim().to_string();
    pair.dest = pair.dest.trim().to_string();
    if pair.name.is_empty() && !pair.source.is_empty() {
        pair.name = Path::new(&pair.source)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| pair.source.clone());
    }
    pair.include = pair
        .include
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    pair.exclude = pair
        .exclude
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
}

pub(crate) fn validate_config(config: &Config) -> Result<(), String> {
    let mut ids: Vec<String> = Vec::new();
    for p in &config.pairs {
        let name = if p.name.is_empty() {
            crate::msg::unnamed_pair().to_string()
        } else {
            p.name.clone()
        };
        let src = &p.source;
        if p.source.is_empty() {
            return Err(crate::msg!("「{name}」未选择源目录", "'{name}' source directory not selected"));
        }
        if p.dest.is_empty() {
            return Err(crate::msg!("「{name}」未选择目标目录", "'{name}' destination directory not selected"));
        }
        if !Path::new(&p.source).is_dir() {
            return Err(crate::msg!(
                "「{name}」源目录不存在: {src}",
                "'{name}' source directory not found: {src}"
            ));
        }
        let s = normalize_path_str(&p.source);
        let d = normalize_path_str(&p.dest);
        if s == d {
            return Err(crate::msg!(
                "「{name}」源目录与目标目录不能相同",
                "'{name}' source and destination cannot be the same"
            ));
        }
        if d.starts_with(&(s.clone() + "\\")) || s.starts_with(&(d.clone() + "\\")) {
            return Err(crate::msg!(
                "「{name}」源目录与目标目录不能互相包含",
                "'{name}' source and destination cannot contain each other"
            ));
        }
        if crate::model::dest_root_unsafe(p) {
            return Err(crate::msg!(
                "「{name}」目标目录不能是盘符根目录（如 F:\\），请指定具体的备份子文件夹（如 F:\\备份\\文档）；整盘对整盘备份时源与目标都选盘符根",
                "'{name}' destination cannot be a drive root (e.g. F:\\). Choose a subfolder such as F:\\Backup (or select drive roots for both sides for a full-drive backup)"
            ));
        }
        Filter::validate(&p.include, &p.exclude)?;
        if let Some(m) = p.interval_minutes {
            if !(1..=10_080).contains(&m) {
                return Err(crate::msg!(
                    "「{name}」定时同步覆盖需在 1 ~ 10080 分钟之间",
                    "'{name}' interval override must be between 1 and 10080 minutes"
                ));
            }
        }
        if ids.contains(&p.id) {
            return Err(crate::msg!("文件夹对标识重复，请重新添加", "Duplicate pair id, please delete and re-add"));
        }
        ids.push(p.id.clone());
    }
    if !(1..=1440).contains(&config.trigger.interval_minutes) {
        return Err(crate::msg!("定时间隔需在 1 ~ 1440 分钟之间", "Interval must be between 1 and 1440 minutes"));
    }
    if !(200..=600_000).contains(&config.trigger.debounce_ms) {
        return Err(crate::msg!("防抖时间需在 0.2 ~ 600 秒之间", "Debounce must be between 0.2 and 600 seconds"));
    }
    if !(1..=3650).contains(&config.version_keep_days) {
        return Err(crate::msg!(
            "版本保留天数需在 1 ~ 3650 天之间",
            "Version keep days must be between 1 and 3650"
        ));
    }
    if config.version_max_count > 10_000 {
        return Err(crate::msg!(
            "每文件保留份数不能超过 10000（0 表示不限）",
            "Max versions per file cannot exceed 10000 (0 = unlimited)"
        ));
    }
    if !(1..=16).contains(&config.copy_threads) {
        return Err(crate::msg!(
            "复制线程数需在 1 ~ 16 之间",
            "Copy threads must be between 1 and 16"
        ));
    }
    if config.after_sync_action == crate::model::AfterSyncAction::Script
        && config.after_sync_script.trim().is_empty()
    {
        return Err(crate::msg!(
            "已选择「执行脚本」，请填写脚本路径",
            "\"Run script\" selected but no script path provided"
        ));
    }
    let ts_fmt = config.version_timestamp_format.trim();
    if ts_fmt.is_empty() || ts_fmt.len() > 64 {
        return Err(crate::msg!(
            "版本时间戳格式需为 1 ~ 64 个字符",
            "Version timestamp format must be 1 to 64 characters"
        ));
    }
    if ts_fmt.chars().any(|c| "<>\"/\\|?*".contains(c)) {
        return Err(crate::msg!(
            "版本时间戳格式不能包含文件名非法字符（<>:\"/\\|?*）",
            "Version timestamp format must not contain filename-invalid characters (<>:\"/\\|?*)"
        ));
    }
    if chrono::format::StrftimeItems::new(ts_fmt)
        .any(|i| matches!(i, chrono::format::Item::Error))
    {
        return Err(crate::msg!(
            "版本时间戳格式无效（chrono 格式串示例：%Y-%m-%d_%H-%M-%S）",
            "Invalid version timestamp format (chrono example: %Y-%m-%d_%H-%M-%S)"
        ));
    }
    Ok(())
}

/// 测试样例相对路径在当前包含/排除规则下是否参与同步
#[tauri::command]
pub fn test_filter(
    include: Vec<String>,
    exclude: Vec<String>,
    path: String,
) -> Result<FilterTestDto, String> {
    use crate::core::filter::is_reserved_root;
    let filter = Filter::new(&include, &exclude)?;
    let rel = path.trim().replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    if rel.is_empty() {
        return Err(crate::msg!("请输入相对路径", "Enter a relative path"));
    }
    if is_reserved_root(rel) {
        return Ok(FilterTestDto {
            matched: false,
            reason: crate::msg!("保留目录，永不参与同步", "Reserved directory — never synced"),
        });
    }
    if !filter.allow_dir(rel) {
        return Ok(FilterTestDto {
            matched: false,
            reason: crate::msg!("命中排除规则，跳过", "Matched an exclude rule — skipped"),
        });
    }
    if !filter.allow_file(rel) {
        return Ok(FilterTestDto {
            matched: false,
            reason: crate::msg!("未命中包含规则，跳过", "Did not match any include rule — skipped"),
        });
    }
    Ok(FilterTestDto {
        matched: true,
        reason: crate::msg!("将参与同步", "Will be synced"),
    })
}

/// CSV 字段转义（RFC 4180）
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 构建历史日志 CSV（含 UTF-8 BOM，Excel 可直接打开）
pub(crate) fn build_history_csv(rows: &[crate::model::LogRow]) -> String {
    let mut out = String::from("\u{FEFF}");
    out.push_str(
        "id,pair_id,pair_name,started_at,finished_at,status,added,updated,deleted,versioned,skipped,failed,trigger,message\r\n",
    );
    for r in rows {
        out.push_str(&r.id.to_string());
        out.push(',');
        out.push_str(&csv_field(&r.pair_id));
        out.push(',');
        out.push_str(&csv_field(&r.pair_name));
        out.push(',');
        out.push_str(&csv_field(&r.started_at));
        out.push(',');
        out.push_str(&csv_field(&r.finished_at));
        out.push(',');
        out.push_str(&csv_field(&r.status));
        out.push(',');
        out.push_str(&r.added.to_string());
        out.push(',');
        out.push_str(&r.updated.to_string());
        out.push(',');
        out.push_str(&r.deleted.to_string());
        out.push(',');
        out.push_str(&r.versioned.to_string());
        out.push(',');
        out.push_str(&r.skipped.to_string());
        out.push(',');
        out.push_str(&r.failed.to_string());
        out.push(',');
        out.push_str(&csv_field(&r.trigger));
        out.push(',');
        out.push_str(&csv_field(&r.message));
        out.push_str("\r\n");
    }
    out
}

/// 导出历史日志到 CSV 文件，返回导出行数
#[tauri::command]
pub fn export_history(path: String, rows: Vec<crate::model::LogRow>) -> Result<usize, String> {
    if rows.is_empty() {
        return Err(crate::msg!("没有可导出的记录", "No records to export"));
    }
    let csv = build_history_csv(&rows);
    std::fs::write(&path, csv)
        .map_err(|e| crate::msg!("写入文件失败：{}", "Failed to write file: {}", e))?;
    Ok(rows.len())
}

/// 暂停/恢复当前同步（文件粒度：已完成文件不受影响，下个文件等待恢复）
#[tauri::command]
pub fn set_sync_paused(app: tauri::AppHandle, paused: bool) {
    app.state::<AppCtx>()
        .syncer
        .paused
        .store(paused, std::sync::atomic::Ordering::SeqCst);
}

/// 按配置应用开机自启动（失败仅记录，不影响保存）
pub(crate) fn apply_autostart(app: &tauri::AppHandle, enable: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    let r = if enable { mgr.enable() } else { mgr.disable() };
    if let Err(e) = r {
        log::warn!("failed to update autostart: {e}");
    }
}

#[tauri::command]
pub fn get_state(app: tauri::AppHandle) -> Result<StateDto, String> {
    runtime::state_dto(&app)
}

#[tauri::command]
pub fn get_app_info(app: tauri::AppHandle) -> AppInfoDto {
    let ctx = app.state::<AppCtx>();
    AppInfoDto {
        portable: ctx.portable,
        data_dir: ctx.data_dir.to_string_lossy().to_string(),
        exe_path: std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

#[tauri::command]
pub async fn save_config(app: tauri::AppHandle, config: Config) -> Result<StateDto, String> {
    let mut config = config;
    for p in &mut config.pairs {
        normalize_pair(p);
    }
    // 校验提示使用新语言；设置立即生效于本进程消息与开机自启动
    crate::msg::set(config.language);
    validate_config(&config)?;

    let config_path = app.state::<AppCtx>().config_path.clone();
    let prev_autostart;
    {
        let ctx = app.state::<AppCtx>();
        let mut cfg = ctx.config.lock().unwrap_or_else(|e| e.into_inner());
        prev_autostart = cfg.autostart;
        *cfg = config.clone();
    }
    storage::config::save(&config_path, &config)?;
    if config.autostart != prev_autostart {
        apply_autostart(&app, config.autostart);
    }
    trigger::refresh_watchers(&app);
    runtime::state_dto(&app)
}

#[tauri::command(async)]
pub fn preview_sync(app: tauri::AppHandle, request: PreviewRequest) -> Result<PreviewDto, String> {
    runtime::preview(&app, request.pair_id)
}

#[tauri::command]
pub fn start_sync(app: tauri::AppHandle, request: SyncRequest) -> Result<(), String> {
    let ids = match request.pair_id {
        Some(id) => vec![id],
        None => Vec::new(),
    };
    runtime::request_sync(&app, ids, request.trigger.as_deref().unwrap_or("manual"));
    Ok(())
}

#[tauri::command]
pub fn get_history(app: tauri::AppHandle, limit: Option<u32>) -> Result<Vec<LogRow>, String> {
    let ctx = app.state::<AppCtx>();
    let db = ctx.db.lock().unwrap_or_else(|e| e.into_inner());
    db.history(limit.unwrap_or(300))
}

#[tauri::command]
pub fn clear_history(app: tauri::AppHandle) -> Result<(), String> {
    let ctx = app.state::<AppCtx>();
    let db = ctx.db.lock().unwrap_or_else(|e| e.into_inner());
    db.clear_history()
}

#[tauri::command(async)]
pub fn cleanup_versions(app: tauri::AppHandle, request: CleanupRequest) -> Result<u32, String> {
    let config = app
        .state::<AppCtx>()
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let keep = config.version_keep_days;
    let pairs: Vec<crate::model::PairConfig> = match &request.pair_id {
        Some(id) => config.pairs.iter().filter(|p| &p.id == id).cloned().collect(),
        None => config.pairs.clone(),
    };
    let mut total = 0u32;
    for p in pairs {
        if !drive_available(&p.dest) {
            continue;
        }
        total += version::cleanup(Path::new(&p.dest), keep)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Config, PairConfig};

    #[test]
    fn dest_drive_root_rejected_unless_source_is_drive_root() {
        let src = std::env::temp_dir().to_string_lossy().to_string();

        // 目标为盘符根、源为普通目录 → 拒绝
        let mut cfg = Config::default();
        cfg.pairs = vec![PairConfig {
            name: "t".into(),
            source: src.clone(),
            dest: "F:\\".into(),
            ..Default::default()
        }];
        assert!(validate_config(&cfg).is_err());

        // 整盘对整盘（源也是盘符根）→ 放行
        let drive_root = format!("{}\\", &src[..3]);
        cfg.pairs[0].source = drive_root;
        assert!(validate_config(&cfg).is_ok());

        // 目标为具体子目录 → 放行
        let mut cfg2 = Config::default();
        cfg2.pairs = vec![PairConfig {
            name: "t".into(),
            source: src,
            dest: "F:\\备份".into(),
            ..Default::default()
        }];
        assert!(validate_config(&cfg2).is_ok());
    }

    #[test]
    fn version_keep_days_bounds_enforced() {
        let mut cfg = Config::default();
        cfg.version_keep_days = 0;
        assert!(validate_config(&cfg).is_err());
        cfg.version_keep_days = 1;
        assert!(validate_config(&cfg).is_ok());
        cfg.version_keep_days = 3651;
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn copy_threads_bounds_enforced() {
        let mut cfg = Config::default();
        cfg.copy_threads = 0;
        assert!(validate_config(&cfg).is_err());
        cfg.copy_threads = 1;
        assert!(validate_config(&cfg).is_ok());
        cfg.copy_threads = 17;
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn version_max_count_bounds_enforced() {
        let mut cfg = Config::default();
        cfg.version_max_count = 0; // 不限
        assert!(validate_config(&cfg).is_ok());
        cfg.version_max_count = 10_001;
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn test_filter_reports_match_reasons() {
        let inc = vec!["*.docx".to_string()];
        let exc = vec!["node_modules/**".to_string()];

        // 命中包含、未被排除
        let r = test_filter(inc.clone(), exc.clone(), "项目A\\报告.docx".into()).unwrap();
        assert!(r.matched);

        // 排除优先
        let r2 = test_filter(inc.clone(), exc, "node_modules\\lib\\a.docx".into()).unwrap();
        assert!(!r2.matched);
        assert!(r2.reason.contains("排除") || r2.reason.contains("exclude"));

        // 未命中包含
        let r3 = test_filter(inc, vec![], "a.txt".into()).unwrap();
        assert!(!r3.matched);

        // 保留目录
        let r4 = test_filter(vec![], vec![], "$RECYCLE.BIN\\x.txt".into()).unwrap();
        assert!(!r4.matched);
        assert!(r4.reason.contains("保留") || r4.reason.contains("Reserved"));

        // 空路径报错
        assert!(test_filter(vec![], vec![], "  ".into()).is_err());
    }

    #[test]
    fn history_csv_escapes_and_formats() {
        use crate::model::LogRow;
        let row = LogRow {
            id: 7,
            pair_id: "p1".into(),
            pair_name: "文档, \"A\"".into(),
            started_at: "2026-10-09 01:00:00".into(),
            finished_at: "2026-10-09 01:00:05".into(),
            status: "ok".into(),
            added: 2,
            updated: 1,
            deleted: 0,
            versioned: 0,
            skipped: 0,
            failed: 0,
            trigger: "manual".into(),
            message: "第一行\n第二行".into(),
        };
        let csv = build_history_csv(&[row]);
        assert!(csv.starts_with("\u{FEFF}"));
        assert!(csv.contains(
            "id,pair_id,pair_name,started_at,finished_at,status,added,updated,deleted,versioned,skipped,failed,trigger,message\r\n"
        ));
        assert!(csv.contains("\"文档, \"\"A\"\"\""));
        assert!(csv.contains("\"第一行\n第二行\""));
        assert!(csv.contains("7,p1,"));
        // 空导出被拒（命令层校验）
        let empty: Vec<LogRow> = vec![];
        assert!(build_history_csv(&empty).ends_with("\r\n"));
    }

    #[test]
    fn after_action_and_ts_format_validated() {
        let mut cfg = Config::default();
        // 基线配置可通过
        assert!(validate_config(&cfg).is_ok());

        // 「执行脚本」必须填写路径
        cfg.after_sync_action = crate::model::AfterSyncAction::Script;
        cfg.after_sync_script = "   ".into();
        assert!(validate_config(&cfg).is_err());
        cfg.after_sync_script = "D:\\x\\run.ps1".into();
        assert!(validate_config(&cfg).is_ok());

        cfg.after_sync_action = crate::model::AfterSyncAction::Shutdown;
        assert!(validate_config(&cfg).is_ok());

        // 时间戳格式：空、过长、非法字符、无效格式串
        cfg.version_timestamp_format = String::new();
        assert!(validate_config(&cfg).is_err());
        cfg.version_timestamp_format = "%Y".repeat(40); // 80 字符 > 64
        assert!(validate_config(&cfg).is_err());
        cfg.version_timestamp_format = "%Y/%m/%d".into();
        assert!(validate_config(&cfg).is_err());
        cfg.version_timestamp_format = "%Q".into();
        assert!(validate_config(&cfg).is_err());
        cfg.version_timestamp_format = "%Y-%m-%d_%H-%M-%S".into();
        assert!(validate_config(&cfg).is_ok());
    }
}
