use crate::core::filter::Filter;
use crate::core::{copy, diff, drive_available, scan, version, Change, DiffResult, FileMeta};
use crate::model::*;
use crate::storage::db::NewLog;
use crate::AppCtx;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

pub fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ---------- 同步请求与工作线程 ----------

#[derive(Clone)]
pub struct SyncReq {
    pub pair_ids: Vec<String>,
    pub trigger: String,
}

#[derive(Clone)]
pub struct Syncer {
    tx: Sender<SyncReq>,
    pub running: Arc<AtomicBool>,
    pub queued: Arc<AtomicBool>,
    /// 暂停标志：置位时工作线程在文件粒度等待，恢复后继续
    pub paused: Arc<AtomicBool>,
}

impl Syncer {
    pub fn spawn(app: tauri::AppHandle) -> Self {
        let (tx, rx) = channel::<SyncReq>();
        let running = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        {
            let running = running.clone();
            let queued = queued.clone();
            std::thread::spawn(move || worker_loop(app, rx, running, queued));
        }
        Self { tx, running, queued, paused }
    }

    pub fn request(&self, req: SyncReq) {
        if self.running.load(Ordering::SeqCst) {
            self.queued.store(true, Ordering::SeqCst);
        }
        let _ = self.tx.send(req);
    }
}

fn worker_loop(
    app: tauri::AppHandle,
    rx: Receiver<SyncReq>,
    running: Arc<AtomicBool>,
    queued: Arc<AtomicBool>,
) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        // 短窗口合并重复触发（实时监控的事件风暴）
        std::thread::sleep(Duration::from_millis(80));
        while let Ok(r) = rx.try_recv() {
            batch.push(r);
        }
        let (ids, trigger) = merge_batch(batch);
        running.store(true, Ordering::SeqCst);
        queued.store(false, Ordering::SeqCst);
        run_sync(&app, &ids, &trigger);
        running.store(false, Ordering::SeqCst);
    }
}

fn merge_batch(batch: Vec<SyncReq>) -> (Vec<String>, String) {
    let mut all = false;
    let mut has_manual = false;
    let mut has_realtime = false;
    let mut ids: Vec<String> = Vec::new();
    for r in batch {
        if r.pair_ids.is_empty() {
            all = true;
        }
        for id in r.pair_ids {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        match r.trigger.as_str() {
            "manual" => has_manual = true,
            "realtime" => has_realtime = true,
            _ => {}
        }
    }
    let trigger = if has_manual {
        "manual"
    } else if has_realtime {
        "realtime"
    } else {
        "interval"
    };
    (if all { Vec::new() } else { ids }, trigger.to_string())
}

// ---------- 对外入口 ----------

pub fn request_sync(app: &tauri::AppHandle, pair_ids: Vec<String>, trigger: &str) {
    let ctx = app.state::<AppCtx>();
    ctx.syncer.request(SyncReq { pair_ids, trigger: trigger.to_string() });
}

pub fn state_dto(app: &tauri::AppHandle) -> Result<StateDto, String> {
    let ctx = app.state::<AppCtx>();
    let config = ctx.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut pair_states = Vec::new();
    {
        let db = ctx.db.lock().unwrap_or_else(|e| e.into_inner());
        for p in &config.pairs {
            let st = db.pair_state(&p.id)?;
            pair_states.push(st.unwrap_or_else(|| PairStateDto {
                pair_id: p.id.clone(),
                pair_name: p.name.clone(),
                ..Default::default()
            }));
        }
    }
    Ok(StateDto {
        config,
        pair_states,
        syncing: ctx.syncer.running.load(Ordering::SeqCst),
        queued: ctx.syncer.queued.load(Ordering::SeqCst),
    })
}

/// 预览返回的最大变更条数（counts/total_bytes 仍按全量统计，前端提示截断）
const PREVIEW_MAX_CHANGES: usize = 1000;

pub fn preview(app: &tauri::AppHandle, pair_id: Option<String>) -> Result<PreviewDto, String> {
    let config = app
        .state::<AppCtx>()
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    let pairs: Vec<&PairConfig> = match &pair_id {
        Some(id) => config.pairs.iter().filter(|p| &p.id == id).collect(),
        None => config.pairs.iter().filter(|p| p.enabled).collect(),
    };

    let mut dto = PreviewDto {
        changes: Vec::new(),
        counts: Counts::default(),
        total_bytes: 0,
        suppressed: 0,
        target_unavailable: false,
        errors: Vec::new(),
    };

    for pair in pairs {
        if !drive_available(&pair.dest) {
            dto.target_unavailable = true;
            dto.errors.push(crate::msg::err_pair_target_unavailable(&pair.name));
            continue;
        }
        if !Path::new(&pair.source).is_dir() {
            dto.errors.push(crate::msg::err_pair_source_missing(&pair.name));
            continue;
        }
        if crate::model::dest_root_unsafe(pair) {
            dto.errors.push(crate::msg::err_pair_dest_root(&pair.name));
            continue;
        }
        match compute_changes(Some(app), pair, &config) {
            Ok((_src, _dst, d, skipped)) => {
                dto.suppressed += d.suppressed;
                if skipped > 0 {
                    dto.errors.push(crate::msg!(
                        "扫描跳过 {skipped} 个不可访问路径",
                        "Scan skipped {skipped} inaccessible path(s)"
                    ));
                }
                for ch in &d.changes {
                    match ch.kind {
                        ChangeKind::Add => dto.counts.added += 1,
                        ChangeKind::Update => dto.counts.updated += 1,
                        ChangeKind::Delete => dto.counts.deleted += 1,
                        ChangeKind::Versioned => dto.counts.versioned += 1,
                    }
                    if matches!(ch.kind, ChangeKind::Add | ChangeKind::Update) {
                        dto.total_bytes += ch.size;
                    }
                    if dto.changes.len() >= PREVIEW_MAX_CHANGES {
                        continue;
                    }
                    dto.changes.push(ChangeDto {
                        pair_id: pair.id.clone(),
                        pair_name: pair.name.clone(),
                        kind: ch.kind,
                        rel_path: ch.rel.clone(),
                        size: ch.size,
                    });
                }
            }
            Err(e) => dto.errors.push(format!(
                "{}{}{e}",
                pair.name,
                crate::msg::pair_err_sep()
            )),
        }
    }

    Ok(dto)
}

// ---------- 同步执行 ----------

struct PairOutcome {
    counts: Counts,
    errors: Vec<String>,
}

fn run_sync(app: &tauri::AppHandle, ids: &[String], trigger: &str) {
    let config = app
        .state::<AppCtx>()
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let force_log = trigger == "manual";

    let pairs: Vec<PairConfig> = config
        .pairs
        .iter()
        .filter(|p| p.enabled && (ids.is_empty() || ids.iter().any(|i| i == &p.id)))
        .cloned()
        .collect();

    let started = Instant::now();
    // 新一轮同步开始时清除暂停标志（上一轮的暂停不遗留）
    app.state::<AppCtx>()
        .syncer
        .paused
        .store(false, Ordering::SeqCst);
    let _ = app.emit(
        "sync-started",
        StartedEvt { trigger: trigger.to_string(), pairs: pairs.len() },
    );

    let mut total = Counts::default();
    let mut failures_total = 0usize;
    let mut target_unavailable = false;
    let mut ok = true;
    let mut run_errors: Vec<String> = Vec::new();

    for pair in &pairs {
        let started_at = now_str();

        if !drive_available(&pair.dest) {
            target_unavailable = true;
            let skip_msg = crate::msg!("目标盘不可用，已跳过", "Target unavailable, skipped");
            record_result(
                app,
                pair,
                &started_at,
                "skipped",
                &Counts::default(),
                trigger,
                &skip_msg,
                force_log,
            );
            continue;
        }
        if !Path::new(&pair.source).is_dir() {
            ok = false;
            let msg = crate::msg!("源目录不存在", "Source directory not found");
            record_result(app, pair, &started_at, "failed", &Counts::default(), trigger, &msg, true);
            run_errors.push(format!("{}{}{msg}", pair.name, crate::msg::pair_err_sep()));
            continue;
        }
        if crate::model::dest_root_unsafe(pair) {
            ok = false;
            let msg = crate::msg::err_pair_dest_root(&pair.name);
            record_result(app, pair, &started_at, "failed", &Counts::default(), trigger, &msg, true);
            run_errors.push(msg);
            continue;
        }
        let _ = std::fs::create_dir_all(&pair.dest);

        match sync_pair(app, pair, &config, &started_at, trigger, force_log) {
            Ok(outcome) => {
                total.added += outcome.counts.added;
                total.updated += outcome.counts.updated;
                total.deleted += outcome.counts.deleted;
                total.versioned += outcome.counts.versioned;
                total.skipped += outcome.counts.skipped;
                total.failed += outcome.counts.failed;
                if outcome.counts.failed > 0 {
                    ok = false;
                    failures_total += outcome.counts.failed as usize;
                    for e in outcome.errors.iter().take(2) {
                        run_errors.push(e.clone());
                    }
                }
            }
            Err(e) => {
                ok = false;
                record_result(app, pair, &started_at, "failed", &Counts::default(), trigger, &e, true);
                run_errors.push(format!("{}{}{e}", pair.name, crate::msg::pair_err_sep()));
            }
        }
    }

    let mut message = String::new();
    if target_unavailable {
        message.push_str(&crate::msg!("目标盘不可用；", "Target unavailable; "));
    }
    message.push_str(&crate::msg::summary(&total));
    if failures_total > 0 {
        message.push_str(&crate::msg!("；失败 {failures_total} 个", "; {failures_total} failed"));
    }
    if !run_errors.is_empty() {
        let item_sep = if crate::msg::en() { "; " } else { "，" };
        message.push_str(&format!(
            "{}{}",
            crate::msg::err_list_sep(),
            run_errors.join(item_sep)
        ));
    }
    if message.len() > 500 {
        message.truncate(500);
    }

    let evt = FinishedEvt {
        ok,
        trigger: trigger.to_string(),
        pairs: pairs.len(),
        counts: total,
        failures: failures_total,
        target_unavailable,
        duration_ms: started.elapsed().as_millis() as u64,
        message,
    };
    let _ = app.emit("sync-finished", evt.clone());
    notify_finish(app, &config, &evt);

    // 全部成功（无失败、目标盘可用、至少执行了一个文件夹对）才执行同步后动作
    if ok && failures_total == 0 && !target_unavailable && !pairs.is_empty() {
        run_after_action(&config);
    }
}

/// 同步全部成功后的附加动作（脚本 / 关机），仅记录日志、不阻塞同步线程
pub(crate) fn run_after_action(config: &Config) {
    match config.after_sync_action {
        AfterSyncAction::None => {}
        AfterSyncAction::Script => {
            let script = config.after_sync_script.trim();
            if script.is_empty() {
                log::warn!("after-sync action skipped: empty script path");
                return;
            }
            let (prog, args) = resolve_script_command(script);
            match std::process::Command::new(&prog).args(&args).spawn() {
                Ok(_) => log::info!("after-sync script launched: {script}"),
                Err(e) => log::warn!("after-sync script failed to launch {script}: {e}"),
            }
        }
        AfterSyncAction::Shutdown => {
            let r = if cfg!(windows) {
                std::process::Command::new("shutdown")
                    .args(["/s", "/t", "60", "/c", "vineSync after-sync"])
                    .spawn()
            } else {
                std::process::Command::new("shutdown").args(["-h", "now"]).spawn()
            };
            match r {
                Ok(_) => log::info!("after-sync shutdown scheduled (60s grace)"),
                Err(e) => log::warn!("after-sync shutdown failed: {e}"),
            }
        }
    }
}

/// 按扩展名选择脚本启动方式：`.ps1` 走 PowerShell、`.bat`/`.cmd` 走 cmd /C，其余直接执行
pub(crate) fn resolve_script_command(script: &str) -> (String, Vec<String>) {
    let ext = Path::new(script)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ps1" => (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                script.to_string(),
            ],
        ),
        "bat" | "cmd" => ("cmd.exe".to_string(), vec!["/C".to_string(), script.to_string()]),
        _ => (script.to_string(), Vec::new()),
    }
}

fn sync_pair(
    app: &tauri::AppHandle,
    pair: &PairConfig,
    config: &Config,
    started_at: &str,
    trigger: &str,
    force_log: bool,
) -> Result<PairOutcome, String> {
    let (src_map, _dst_map, d, skipped) = compute_changes(Some(app), pair, config)?;
    let total = d.changes.len();
    let mut counts = Counts { skipped: d.suppressed, ..Counts::default() };
    let mut errors: Vec<String> = Vec::new();
    let mut last_emit = Instant::now() - Duration::from_millis(500);

    let threads = config.copy_threads.clamp(1, 16) as usize;
    let mut bytes_done: u64 = 0;
    let paused = app.state::<AppCtx>().syncer.paused.clone();
    let (applied, errs) =
        apply_changes(pair, config, &d.changes, threads, &paused, |done, ch| {
        if matches!(ch.kind, ChangeKind::Add | ChangeKind::Update) {
            bytes_done += ch.size;
        }
        if last_emit.elapsed() >= Duration::from_millis(120) || done == total {
            last_emit = Instant::now();
            let _ = app.emit(
                "sync-progress",
                ProgressEvt {
                    pair_name: pair.name.clone(),
                    rel_path: ch.rel.clone(),
                    action: ch.kind,
                    done,
                    total,
                    bytes: bytes_done,
                },
            );
        }
    });
    counts.added += applied.added;
    counts.updated += applied.updated;
    counts.deleted += applied.deleted;
    counts.versioned += applied.versioned;
    counts.failed += applied.failed;
    errors.extend(errs);

    if skipped > 0 {
        errors.push(crate::msg!(
            "扫描跳过 {skipped} 个不可访问路径",
            "Scan skipped {skipped} inaccessible path(s)"
        ));
    }

    // 同步后自动清理过期版本备份（保留天数取设置值；失败不影响同步结果）
    if let Err(e) = version::cleanup(Path::new(&pair.dest), config.version_keep_days) {
        log::warn!("version backup auto cleanup failed for {}: {e}", pair.name);
    }

    let status = if counts.failed > 0 {
        if counts.total_changes() > 0 {
            "partial"
        } else {
            "failed"
        }
    } else {
        "ok"
    };
    let message = build_message(&counts, &errors);
    record_result(app, pair, started_at, status, &counts, trigger, &message, force_log);

    if counts.failed == 0 && (counts.total_changes() > 0 || snapshot_empty(app, &pair.id)) {
        let ctx = app.state::<AppCtx>();
        let db = ctx.db.lock();
        if let Ok(db) = db {
            let _ = db.sync_file_state(&pair.id, &src_map);
        }
    }

    Ok(PairOutcome { counts, errors })
}

fn compute_changes(
    app: Option<&tauri::AppHandle>,
    pair: &PairConfig,
    config: &Config,
) -> Result<DiffResult3, String> {
    let filter = Filter::new(&pair.include, &pair.exclude)?;
    // 超长路径（>260）：根路径转 verbatim，子路径 join 继承前缀
    let src_root = crate::core::to_long_path(Path::new(&pair.source));
    let dst_root = crate::core::to_long_path(Path::new(&pair.dest));

    // 源端扫描：每 250ms 上报一次进度（last rel 复用缓冲区，避免每文件分配）
    let mut src_files = 0u64;
    let mut src_last = Instant::now() - Duration::from_millis(250);
    let mut src_last_rel = String::new();
    let src_out = scan::scan_with_progress(&src_root, &filter, false, |rel| {
        src_files += 1;
        src_last_rel.clear();
        src_last_rel.push_str(rel);
        if src_last.elapsed() >= Duration::from_millis(250) {
            src_last = Instant::now();
            emit_scan(app, pair, "source", src_files, rel);
        }
    })?;
    emit_scan(app, pair, "source", src_files, &src_last_rel);

    // 快速扫描（差分）：开启 diff_scan 且该文件夹对上次同步成功（status=ok）时，
    // 目标端清单直接取上次成功同步的 file_state 快照，跳过目标目录遍历。
    // 前提：复制会把目标文件的大小/mtime 对齐源文件，快照即可代表目标状态。
    // 代价：目标端的外部改动（手工增删）在下次完整扫描前不可见。
    let fast_scan = match app {
        Some(a) if config.diff_scan => {
            let ctx = a.state::<AppCtx>();
            let last_ok = ctx
                .db
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pair_state(&pair.id)
                .ok()
                .flatten()
                .map(|s| s.last_status.as_deref() == Some("ok"))
                .unwrap_or(false);
            last_ok
        }
        _ => false,
    };

    let (mut dst_map, dst_skipped) = match (fast_scan, app) {
        (true, Some(a)) => {
            let ctx = a.state::<AppCtx>();
            let snap = ctx
                .db
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .load_file_state(&pair.id)?;
            emit_scan(app, pair, "target", snap.len() as u64, "");
            (snap, 0usize)
        }
        _ => {
            // 目标端扫描（清理临时文件）
            let mut dst_files = 0u64;
            let mut dst_last = Instant::now() - Duration::from_millis(250);
            let mut dst_last_rel = String::new();
            let dst_out = scan::scan_with_progress(&dst_root, &filter, true, |rel| {
                dst_files += 1;
                dst_last_rel.clear();
                dst_last_rel.push_str(rel);
                if dst_last.elapsed() >= Duration::from_millis(250) {
                    dst_last = Instant::now();
                    emit_scan(app, pair, "target", dst_files, rel);
                }
            })?;
            emit_scan(app, pair, "target", dst_files, &dst_last_rel);
            (dst_out.files, dst_out.skipped.len())
        }
    };

    // 源端不可见的路径同步从目标清单剔除，避免被误判为“已删除”而清理副本
    let skipped = src_out.skipped.len() + dst_skipped;
    scan::prune_under(&mut dst_map, &src_out.skipped);
    let src_map = src_out.files;

    let d = diff(
        &src_map,
        &dst_map,
        config.delete_policy,
        config.version_on_delete,
        config.hash_verify,
        &src_root,
        &dst_root,
    );
    Ok((src_map, dst_map, d, skipped))
}

fn emit_scan(app: Option<&tauri::AppHandle>, pair: &PairConfig, phase: &str, files: u64, rel: &str) {
    if let Some(app) = app {
        let _ = app.emit(
            "scan-progress",
            ScanProgressEvt {
                pair_name: pair.name.clone(),
                phase: phase.to_string(),
                files,
                rel_path: rel.to_string(),
            },
        );
    }
}

/// (源清单, 目标清单(已剔除源端不可见路径), 差异, 扫描跳过的不可访问路径数)
type DiffResult3 = (
    BTreeMap<String, FileMeta>,
    BTreeMap<String, FileMeta>,
    DiffResult,
    usize,
);

fn apply_change(pair: &PairConfig, ch: &Change, config: &Config) -> Result<(), String> {
    let src_root = crate::core::to_long_path(Path::new(&pair.source));
    let dst_root = crate::core::to_long_path(Path::new(&pair.dest));
    let policy = config.delete_policy;
    match ch.kind {
        ChangeKind::Add => {
            let rel = ch
                .src_rel
                .as_ref()
                .ok_or_else(|| crate::msg!("内部错误：缺少源路径", "Internal error: missing source path"))?;
            let src = src_root.join(rel);
            let dst = dst_root.join(rel);
            copy::safe_copy(&src, &dst, file_mtime_ns(&src))?;
            if config.verify_after_copy {
                verify_copy_hash(&src, &dst)?;
            }
            Ok(())
        }
        ChangeKind::Update => {
            let rel = ch
                .src_rel
                .as_ref()
                .ok_or_else(|| crate::msg!("内部错误：缺少源路径", "Internal error: missing source path"))?;
            let src = src_root.join(rel);
            let dst = dst_root.join(rel);
            let mtime_ns = file_mtime_ns(&src);
            // 策略 B（纯镜像）始终不备份；A/C 受「覆盖前备份」开关控制
            let r = if policy != DeletePolicy::Mirror && config.version_on_replace {
                if let Some(dst_rel) = &ch.dst_rel {
                    // 新文件先暂存并校验通过后，才把旧文件移入版本备份，最后原子改名
                    let dst_rel = dst_rel.clone();
                    let max_count = config.version_max_count;
                    let ts_fmt = config.version_timestamp_format.clone();
                    copy::safe_copy_hook(&src, &dst, mtime_ns, move || {
                        version::move_to_version(&dst_root, &dst_rel, max_count, &ts_fmt)
                            .map(|_| ())
                    })
                } else {
                    copy::safe_copy(&src, &dst, mtime_ns)
                }
            } else {
                copy::safe_copy(&src, &dst, mtime_ns)
            };
            if r.is_ok() && config.verify_after_copy {
                verify_copy_hash(&src, &dst)?;
            }
            r
        }
        ChangeKind::Delete => {
            let dst_rel = ch
                .dst_rel
                .as_ref()
                .ok_or_else(|| crate::msg!("内部错误：缺少目标路径", "Internal error: missing destination path"))?;
            let dst = dst_root.join(dst_rel);
            if dst.exists() {
                std::fs::remove_file(&dst)
                    .map_err(|e| crate::msg!("删除失败: {e}", "Delete failed: {e}"))?;
            }
            Ok(())
        }
        ChangeKind::Versioned => {
            let dst_rel = ch
                .dst_rel
                .as_ref()
                .ok_or_else(|| crate::msg!("内部错误：缺少目标路径", "Internal error: missing destination path"))?;
            version::move_to_version(&dst_root, dst_rel, config.version_max_count, &config.version_timestamp_format)
                .map(|_| ())
        }
    }
}

/// 深度校验：回读源与目标并比对哈希（失败进入重试）
fn verify_copy_hash(src: &Path, dst: &Path) -> Result<(), String> {
    let h1 = crate::core::file_hash(src).map_err(|e| {
        crate::msg!("深度校验读取源文件失败: {e}", "Deep verify: read source failed: {e}")
    })?;
    let h2 = crate::core::file_hash(dst).map_err(|e| {
        crate::msg!("深度校验读取目标文件失败: {e}", "Deep verify: read destination failed: {e}")
    })?;
    if h1 != h2 {
        return Err(crate::msg!(
            "深度校验失败（目标与源哈希不一致）: {}",
            "Deep verify failed (hash mismatch): {}",
            dst.display()
        ));
    }
    Ok(())
}

fn file_mtime_ns(src: &Path) -> i64 {
    std::fs::metadata(src)
        .and_then(|m| m.modified())
        .map(crate::core::systemtime_to_ns)
        .unwrap_or(0)
}

// ---------- 变更执行（顺序 / 多线程并行 + 自动重试） ----------

/// 失败自动重试：最多 3 次尝试、递增延迟，缓解瞬时文件占用与网络抖动
fn apply_change_retry(pair: &PairConfig, ch: &Change, config: &Config) -> Result<(), String> {
    let mut attempt = 0u32;
    loop {
        match apply_change(pair, ch, config) {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempt += 1;
                if attempt >= 3 {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(if attempt == 1 { 150 } else { 350 }));
            }
        }
    }
}

fn bump_kind(counts: &mut Counts, kind: ChangeKind) {
    match kind {
        ChangeKind::Add => counts.added += 1,
        ChangeKind::Update => counts.updated += 1,
        ChangeKind::Delete => counts.deleted += 1,
        ChangeKind::Versioned => counts.versioned += 1,
    }
}

/// 暂停标志置位时在文件粒度等待（每 100ms 检查一次）
fn wait_while_paused(paused: &AtomicBool) {
    while paused.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 执行一组变更：`threads == 1` 或变更很少时顺序执行，否则多线程并行。
/// 每个文件失败自动重试；`paused` 置位时在文件粒度等待恢复；
/// `on_progress(done, ch)` 在调用线程按完成顺序回调。
/// 各变更互不依赖（每条对应不同相对路径），并行是安全的。
fn apply_changes(
    pair: &PairConfig,
    config: &Config,
    changes: &[Change],
    threads: usize,
    paused: &AtomicBool,
    mut on_progress: impl FnMut(usize, &Change),
) -> (Counts, Vec<String>) {
    let mut counts = Counts::default();
    let mut errors: Vec<String> = Vec::new();

    if threads <= 1 || changes.len() < threads * 2 {
        for (i, ch) in changes.iter().enumerate() {
            wait_while_paused(paused);
            match apply_change_retry(pair, ch, config) {
                Ok(()) => bump_kind(&mut counts, ch.kind),
                Err(e) => {
                    counts.failed += 1;
                    errors.push(format!("{}：{e}", ch.rel));
                }
            }
            on_progress(i + 1, ch);
        }
        return (counts, errors);
    }

    let queue: Mutex<VecDeque<usize>> = Mutex::new((0..changes.len()).collect());
    let (tx, rx) = channel::<(usize, Result<(), String>)>();

    let (counts, errors) = std::thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let queue = &queue;
            let paused = paused;
            s.spawn(move || {
                loop {
                    let idx = {
                        let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                        q.pop_front()
                    };
                    let Some(idx) = idx else { break };
                    wait_while_paused(paused);
                    let r = apply_change_retry(pair, &changes[idx], config);
                    if tx.send((idx, r)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut counts = Counts::default();
        let mut errors: Vec<String> = Vec::new();
        let mut done = 0usize;
        for (idx, result) in rx {
            done += 1;
            let ch = &changes[idx];
            match result {
                Ok(()) => bump_kind(&mut counts, ch.kind),
                Err(e) => {
                    counts.failed += 1;
                    errors.push(format!("{}：{e}", ch.rel));
                }
            }
            on_progress(done, ch);
        }
        (counts, errors)
    });
    (counts, errors)
}

// ---------- 状态与日志 ----------

fn record_result(
    app: &tauri::AppHandle,
    pair: &PairConfig,
    started_at: &str,
    status: &str,
    counts: &Counts,
    trigger: &str,
    message: &str,
    force_log: bool,
) {
    let finished_at = now_str();
    let st = PairStateDto {
        pair_id: pair.id.clone(),
        pair_name: pair.name.clone(),
        last_sync_at: Some(finished_at.clone()),
        last_status: Some(status.to_string()),
        added: counts.added,
        updated: counts.updated,
        deleted: counts.deleted,
        versioned: counts.versioned,
        failed: counts.failed,
        message: message.to_string(),
    };
    let ctx = app.state::<AppCtx>();
    let db = match ctx.db.lock() {
        Ok(db) => db,
        Err(e) => e.into_inner(),
    };
    let _ = db.upsert_pair_state(&st);
    let should_log = force_log
        || counts.total_changes() > 0
        || counts.failed > 0
        || status != "ok";
    if should_log {
        let _ = db.insert_log(&NewLog {
            pair_id: pair.id.clone(),
            pair_name: pair.name.clone(),
            started_at: started_at.to_string(),
            finished_at,
            status: status.to_string(),
            counts: counts.clone(),
            trigger: trigger.to_string(),
            message: message.to_string(),
        });
    }
}

fn snapshot_empty(app: &tauri::AppHandle, pair_id: &str) -> bool {
    let ctx = app.state::<AppCtx>();
    let db = match ctx.db.lock() {
        Ok(db) => db,
        Err(e) => e.into_inner(),
    };
    db.file_state_count(pair_id).map(|n| n == 0).unwrap_or(false)
}

fn build_message(counts: &Counts, errors: &[String]) -> String {
    let mut msg = crate::msg::summary(counts);
    if !errors.is_empty() {
        let item_sep = if crate::msg::en() { "; " } else { "，" };
        msg.push_str(&format!(
            "{}{}",
            crate::msg::err_list_sep(),
            errors.join(item_sep)
        ));
        if errors.len() > 2 {
            msg.push_str(crate::msg::err_list_more());
        }
    }
    msg
}

fn notify_finish(app: &tauri::AppHandle, config: &Config, evt: &FinishedEvt) {
    if !config.notify_on_finish {
        return;
    }
    let interesting = evt.counts.total_changes() > 0
        || evt.failures > 0
        || evt.target_unavailable
        || evt.trigger == "manual";
    if !interesting {
        return;
    }
    let title = if evt.ok {
        crate::msg!("vineSync 同步完成", "vineSync sync finished")
    } else {
        crate::msg!("vineSync 同步存在问题", "vineSync sync issues")
    };
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(evt.message.clone())
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_pair(name: &str) -> (std::path::PathBuf, std::path::PathBuf, PairConfig) {
        let base = std::env::temp_dir().join(format!("vs_apply_{name}_{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let pair = PairConfig {
            name: name.into(),
            source: src.to_string_lossy().to_string(),
            dest: dst.to_string_lossy().to_string(),
            ..Default::default()
        };
        (src, dst, pair)
    }

    fn add_change(rel: &str) -> Change {
        Change {
            kind: ChangeKind::Add,
            rel: rel.to_string(),
            size: 0,
            src_rel: Some(rel.to_string()),
            dst_rel: None,
        }
    }

    #[test]
    fn apply_changes_parallel_copies_all_files() {
        let (src, dst, pair) = tmp_pair("par");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "A").unwrap();
        std::fs::write(src.join("sub\\b.txt"), "BB").unwrap();
        std::fs::write(src.join("c.txt"), "CCC").unwrap();

        let changes = vec![add_change("a.txt"), add_change("sub\\b.txt"), add_change("c.txt")];
        let mut progress: Vec<usize> = Vec::new();
        let (counts, errors) = apply_changes(
            &pair,
            &Config::default(),
            &changes,
            4,
            &AtomicBool::new(false),
            |done, _| {
                progress.push(done);
            },
        );

        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(counts.added, 3);
        assert_eq!(counts.failed, 0);
        assert_eq!(fs_read(&dst.join("a.txt")), "A");
        assert_eq!(fs_read(&dst.join("sub\\b.txt")), "BB");
        assert_eq!(fs_read(&dst.join("c.txt")), "CCC");
        assert_eq!(progress.len(), 3);
        assert_eq!(*progress.last().unwrap(), 3);
        let _ = std::fs::remove_dir_all(base_of(&pair));
    }

    #[test]
    fn apply_changes_records_failures_and_still_progresses() {
        let (src, dst, pair) = tmp_pair("fail");
        std::fs::write(src.join("ok.txt"), "ok").unwrap();

        // src_rel 指向不存在的文件 → 重试 3 次后失败，不影响其余变更
        let changes = vec![
            add_change("missing.txt"),
            Change {
                kind: ChangeKind::Add,
                rel: "ok.txt".into(),
                size: 0,
                src_rel: Some("ok.txt".into()),
                dst_rel: None,
            },
        ];
        let mut last_done = 0usize;
        let (counts, errors) = apply_changes(
            &pair,
            &Config::default(),
            &changes,
            2,
            &AtomicBool::new(false),
            |done, _| {
                last_done = done;
            },
        );

        assert_eq!(counts.failed, 1);
        assert_eq!(counts.added, 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing.txt"));
        assert_eq!(last_done, 2);
        assert_eq!(fs_read(&dst.join("ok.txt")), "ok");
        let _ = std::fs::remove_dir_all(base_of(&pair));
    }

    #[test]
    fn apply_changes_pause_blocks_until_resumed() {
        let (src, dst, pair) = tmp_pair("pause");
        std::fs::write(src.join("a.txt"), "A").unwrap();
        std::fs::write(src.join("b.txt"), "B").unwrap();
        let changes = vec![add_change("a.txt"), add_change("b.txt")];

        let paused = Arc::new(AtomicBool::new(true));
        let flag = paused.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(false, Ordering::SeqCst);
        });
        let start = Instant::now();
        let (counts, errors) =
            apply_changes(&pair, &Config::default(), &changes, 1, &paused, |_, _| {});
        h.join().unwrap();

        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(counts.added, 2);
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "暂停期间不应执行变更"
        );
        assert_eq!(fs_read(&dst.join("a.txt")), "A");
        let _ = std::fs::remove_dir_all(base_of(&pair));
    }

    #[test]
    fn resolve_script_command_by_extension() {
        let (p, a) = resolve_script_command("C:\\x\\run.ps1");
        assert_eq!(p, "powershell.exe");
        assert!(a.contains(&"-File".to_string()));
        assert_eq!(a.last().unwrap(), "C:\\x\\run.ps1");

        let (p, a) = resolve_script_command("C:\\x\\task.bat");
        assert_eq!(p, "cmd.exe");
        assert_eq!(a[0], "/C");

        let (p, a) = resolve_script_command("C:\\x\\tool.exe");
        assert_eq!(p, "C:\\x\\tool.exe");
        assert!(a.is_empty());
    }

    #[test]
    fn verify_copy_hash_detects_mismatch() {
        let (src, dst, pair) = tmp_pair("verify");
        std::fs::write(src.join("a.txt"), "hello").unwrap();
        std::fs::write(dst.join("b.txt"), "other").unwrap();

        assert!(verify_copy_hash(&src.join("a.txt"), &src.join("a.txt")).is_ok());
        assert!(verify_copy_hash(&src.join("a.txt"), &dst.join("b.txt")).is_err());
        let _ = std::fs::remove_dir_all(base_of(&pair));
    }

    fn fs_read(p: &std::path::Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    fn base_of(pair: &PairConfig) -> std::path::PathBuf {
        std::path::Path::new(&pair.source)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    }
}
