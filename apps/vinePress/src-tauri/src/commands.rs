use crate::detect::detect;
use crate::export::{run_export, ExportConfig, ExportProgress};
use crate::notes::{note_text, write_note, NoteInput};
use crate::project::{rename_entry_paths, ArchiveSettings, Entry, Overrides, PasswordProfile, Project};
use crate::scan::scan_tree;
use crate::{fsops, meta};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Serialize)]
pub struct ToolInfo {
    pub rar: Option<String>,
    pub seven_zip: Option<String>,
}

#[derive(Debug, Default)]
pub struct JobState {
    pub cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotesProgress {
    /// unit | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rel_path: String,
    /// running | created | error | cancelled | finished
    pub status: String,
    pub message: String,
    pub written: usize,
    pub failed: usize,
}

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    pub root: String,
    pub output_root: String,
    pub entries: Vec<Entry>,
    pub settings: ArchiveSettings,
    pub profiles: Vec<PasswordProfile>,
}

#[derive(Debug, Serialize)]
pub struct ScanResponse {
    pub tree: Vec<crate::detect::DetectNode>,
    /// 预填默认值的条目（含 work/category 推导），前端合并时保留已编辑字段
    pub entries: Vec<Entry>,
    pub container_count: u32,
    pub ignored_count: u32,
    pub total_size: u64,
    pub total_files: u32,
    /// 根目录直属内容文件（不入任何单位，仅提示）
    pub root_loose: Vec<String>,
}

/// 扫描目录并返回识别结果（容器树 + 预填条目）
#[tauri::command(async)]
pub fn scan_directory(root: String, overrides: Overrides) -> Result<ScanResponse, String> {
    let tree = scan_tree(Path::new(&root))?;
    let mut detected = detect(&tree, &overrides);
    crate::kindx::enrich_cover_matches(Path::new(&root), &mut detected);
    Ok(ScanResponse {
        tree: detected.tree,
        entries: detected.units.iter().map(Entry::from_unit).collect(),
        container_count: detected.container_count,
        ignored_count: detected.ignored_count,
        total_size: detected.total_size,
        total_files: detected.total_files,
        root_loose: detected.root_loose,
    })
}

/// 探测本机可用的压缩工具链
#[tauri::command]
pub fn probe_tools() -> ToolInfo {
    ToolInfo {
        rar: find_rar(),
        seven_zip: find_seven_zip(),
    }
}

#[tauri::command(async)]
pub fn save_project(path: String, project: Project) -> Result<(), String> {
    project.save(Path::new(&path))
}

#[tauri::command(async)]
pub fn load_project(path: String) -> Result<Project, String> {
    Project::load(Path::new(&path))
}

/// 批量生成说明文件（后台线程，通过 notes-progress 事件回报）
#[tauri::command]
pub fn start_generate_notes(
    app: AppHandle,
    state: State<'_, JobState>,
    root: String,
    entries: Vec<Entry>,
) -> Result<(), String> {
    for e in &entries {
        fsops::validate_rel(&e.rel_path)?;
    }
    let (cancel, slot) = begin_job(&state)?;
    let app2 = app.clone();

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            notes_worker(app, root, entries, cancel);
        }));
        if result.is_err() {
            let _ = app2.emit(
                "notes-progress",
                NotesProgress {
                    phase: "finished".into(),
                    index: 0,
                    total: 0,
                    rel_path: String::new(),
                    status: "error".into(),
                    message: "任务异常终止（内部错误），请重试".into(),
                    written: 0,
                    failed: 0,
                },
            );
            eprintln!("[notes] worker panic，任务状态已复位");
        }
        end_job(&slot);
    });

    Ok(())
}

fn notes_worker(
    app: AppHandle,
    root: String,
    entries: Vec<Entry>,
    cancel: Arc<AtomicBool>,
) {
    let root_path = Path::new(&root).to_path_buf();
    let total = entries.len();
    let mut written = 0usize;
    let mut failed = 0usize;

    for (i, entry) in entries.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            let _ = app.emit(
                "notes-progress",
                NotesProgress {
                    phase: "finished".into(),
                    index: i,
                    total,
                    rel_path: entry.rel_path.clone(),
                    status: "cancelled".into(),
                    message: "已取消".into(),
                    written,
                    failed,
                },
            );
            return;
        }

        let mut unit_path = root_path.clone();
        for seg in entry.rel_path.split('/') {
            if !seg.is_empty() {
                unit_path.push(seg);
            }
        }
        let input = NoteInput {
            entry,
            stats: &entry.stats,
            unit_path: &unit_path,
        };
        let progress = NotesProgress {
            phase: "unit".into(),
            index: i + 1,
            total,
            rel_path: entry.rel_path.clone(),
            status: "running".into(),
            message: String::new(),
            written,
            failed,
        };
        let _ = app.emit("notes-progress", &progress);

        match write_note(&input) {
            Ok((path, _)) => {
                written += 1;
                let _ = app.emit(
                    "notes-progress",
                    NotesProgress {
                        phase: "unit".into(),
                        index: i + 1,
                        total,
                        rel_path: entry.rel_path.clone(),
                        status: "created".into(),
                        message: path.to_string_lossy().to_string(),
                        written,
                        failed,
                    },
                );
            }
            Err(e) => {
                failed += 1;
                let _ = app.emit(
                    "notes-progress",
                    NotesProgress {
                        phase: "unit".into(),
                        index: i + 1,
                        total,
                        rel_path: entry.rel_path.clone(),
                        status: "error".into(),
                        message: e,
                        written,
                        failed,
                    },
                );
            }
        }
    }

    let _ = app.emit(
        "notes-progress",
        NotesProgress {
            phase: "finished".into(),
            index: total,
            total,
            rel_path: String::new(),
            status: "finished".into(),
            message: format!("说明文件生成完成：成功 {}、失败 {}", written, failed),
            written,
            failed,
        },
    );
}

/// 批量压缩导出（后台线程，通过 export-progress 事件回报）
#[tauri::command]
pub fn start_export(
    app: AppHandle,
    state: State<'_, JobState>,
    request: ExportRequest,
) -> Result<(), String> {
    for e in &request.entries {
        fsops::validate_rel(&e.rel_path)?;
    }
    // ZIP 用内置 zip 库，不需要 WinRAR
    let rar_path = if request.settings.format == "zip" {
        String::new()
    } else {
        find_rar().ok_or("未找到 WinRAR（Rar.exe），无法导出 RAR")?
    };
    let (cancel, slot) = begin_job(&state)?;
    let app2 = app.clone();

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            export_worker(app, rar_path, request, cancel);
        }));
        if result.is_err() {
            let _ = app2.emit(
                "export-progress",
                ExportProgress {
                    phase: "finished".into(),
                    index: 0,
                    total: 0,
                    rel_path: String::new(),
                    unit_percent: 0,
                    global_percent: 0.0,
                    status: "error".into(),
                    message: "任务异常终止（内部错误），请重试".into(),
                    bytes: 0,
                    created: 0,
                    skipped: 0,
                    failed: 0,
                    archives: vec![],
                },
            );
            eprintln!("[export] worker panic，任务状态已复位");
        }
        end_job(&slot);
    });

    Ok(())
}

fn export_worker(app: AppHandle, rar_path: String, request: ExportRequest, cancel: Arc<AtomicBool>) {
    let cfg = ExportConfig {
        rar_path: &rar_path,
        root: Path::new(&request.root),
        output_root: Path::new(&request.output_root),
        settings: &request.settings,
        profiles: &request.profiles,
        entries: &request.entries,
    };
    run_export(
        &cfg,
        &|p: &ExportProgress| {
            let _ = app.emit("export-progress", p);
        },
        &cancel,
    );
}

/// 取消当前任务
#[tauri::command]
pub fn cancel_job(state: State<'_, JobState>) -> Result<(), String> {
    // 解毒：即使先前有 panic 导致锁中毒，也要能继续使用
    let guard = state.cancel.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(flag) = guard.as_ref() {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

// ---------------------------------------------------------------- 自动识别填充

/// 为若干条目生成元数据「提案」（不改写任何数据）
#[tauri::command(async)]
pub fn auto_fill_proposals(
    root: String,
    entries: Vec<Entry>,
    fields: Vec<String>,
) -> Result<Vec<meta::Proposal>, String> {
    if root.trim().is_empty() {
        return Err("未设置项目根目录".into());
    }
    Ok(meta::propose_all(Path::new(&root), &entries, &fields))
}

// ---------------------------------------------------------------- 文件操作（限根目录内）

#[tauri::command(async)]
pub fn list_dir(root: String, path: String, show_all: bool) -> Result<Vec<fsops::FileItem>, String> {
    fsops::list_dir(&root, &path, show_all)
}

#[tauri::command(async)]
pub fn open_path(root: String, path: String) -> Result<(), String> {
    fsops::open_path(&root, &path)
}

#[tauri::command(async)]
pub fn reveal_path(root: String, path: String) -> Result<(), String> {
    fsops::reveal_path(&root, &path)
}

#[tauri::command(async)]
pub fn make_directory(root: String, path: String) -> Result<(), String> {
    fsops::make_directory(&root, &path)
}

#[tauri::command(async)]
pub fn copy_items(root: String, items: Vec<String>, dest: String) -> Result<Vec<String>, String> {
    fsops::copy_items(&root, &items, &dest)
}

#[tauri::command(async)]
pub fn move_items(root: String, items: Vec<String>, dest: String) -> Result<Vec<String>, String> {
    fsops::move_items(&root, &items, &dest)
}

#[derive(Debug, Serialize)]
pub struct RenameOutcome {
    /// 改名后的相对路径（不在根目录内时为空串）
    pub new_rel_path: String,
    /// 已同步 rel_path / name 的条目列表
    pub entries: Vec<Entry>,
}

/// 磁盘重命名（目录或文件）+ 条目 rel_path 同步。
/// 单位改名后：压缩包名 ≡ 目录名 ≡ 说明文件名。
#[tauri::command(async)]
pub fn rename_path(
    root: String,
    entries: Vec<Entry>,
    path: String,
    new_name: String,
) -> Result<RenameOutcome, String> {
    let new_abs = fsops::rename_path(&root, &path, &new_name)?;
    let old_rel = fsops::relative_to(&root, &path).unwrap_or_default();
    let new_rel = fsops::relative_to(&root, &new_abs).unwrap_or_default();
    let mut entries = entries;
    if !old_rel.is_empty() && !new_rel.is_empty() {
        rename_entry_paths(&mut entries, &old_rel, &new_rel);
    }
    Ok(RenameOutcome {
        new_rel_path: new_rel,
        entries,
    })
}

// ---------------------------------------------------------------- 命名检查 / 修复

/// 命名档案下拉项 [(id, 显示名)]
#[tauri::command]
pub fn name_profiles() -> Vec<(String, String)> {
    crate::naming::profile_options()
}

/// 按目标平台/网盘规则检查全盘名称（只读，扫描一次目录树）
#[tauri::command(async)]
pub fn check_names(root: String, profile: String) -> Result<crate::naming::NameCheckResult, String> {
    let tree = scan_tree(Path::new(&root))?;
    Ok(crate::naming::check_tree(&tree, crate::naming::profile_from(&profile)))
}

#[derive(Debug, Deserialize)]
pub struct FixNameItem {
    pub rel_path: String,
    pub new_name: String,
}

#[derive(Debug, Serialize)]
pub struct RenamePair {
    pub old_rel: String,
    pub new_rel: String,
}

#[derive(Debug, Serialize)]
pub struct FixNamesFail {
    pub rel_path: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct FixNamesOutcome {
    /// 已同步 rel_path/name 的条目列表（调用方直接接替项目状态）
    pub entries: Vec<Entry>,
    pub renamed: Vec<RenamePair>,
    pub failed: Vec<FixNamesFail>,
}

/// 批量修复命名：浅层优先磁盘重命名；父目录改名后自动位移剩余待改项的 rel。
/// 说明文件名同步由 `fsops::rename_path` 负责（压缩包名 ≡ 目录名 ≡ 说明文件名）。
#[tauri::command(async)]
pub fn fix_names(
    root: String,
    entries: Vec<Entry>,
    items: Vec<FixNameItem>,
) -> Result<FixNamesOutcome, String> {
    let mut items = items;
    items.sort_by_key(|i| i.rel_path.matches('/').count());
    let mut entries = entries;
    let mut renamed: Vec<RenamePair> = Vec::new();
    let mut failed: Vec<FixNamesFail> = Vec::new();
    for i in 0..items.len() {
        let rel = items[i].rel_path.clone();
        let new_name = items[i].new_name.clone();
        if rel.is_empty() {
            failed.push(FixNamesFail {
                rel_path: rel,
                message: "根目录不能在应用内改名（请在文件管理器中手动改）".into(),
            });
            continue;
        }
        let mut abs = PathBuf::from(&root);
        for seg in rel.split('/') {
            if !seg.is_empty() {
                abs.push(seg);
            }
        }
        let abs_str = abs.to_string_lossy().to_string();
        match fsops::rename_path(&root, &abs_str, &new_name) {
            Ok(new_abs) => {
                let new_rel = fsops::relative_to(&root, &new_abs).unwrap_or_default();
                if !new_rel.is_empty() {
                    rename_entry_paths(&mut entries, &rel, &new_rel);
                }
                // 父目录改名 → 后续待改项（其后代）rel 位移
                let prefix = format!("{}/", rel);
                for later in items.iter_mut().skip(i + 1) {
                    if later.rel_path.starts_with(&prefix) {
                        later.rel_path = format!("{}{}", new_rel, &later.rel_path[rel.len()..]);
                    }
                }
                renamed.push(RenamePair {
                    old_rel: rel,
                    new_rel,
                });
            }
            Err(e) => failed.push(FixNamesFail {
                rel_path: rel,
                message: e,
            }),
        }
    }
    Ok(FixNamesOutcome {
        entries,
        renamed,
        failed,
    })
}

// ---------------------------------------------------------------- 预览 / 导出自检 / 回收站

/// 预览说明文件：只渲染不写盘
#[tauri::command(async)]
pub fn preview_note(root: String, entry: Entry) -> Result<String, String> {
    fsops::validate_rel(&entry.rel_path)?;
    let mut unit_path = Path::new(&root).to_path_buf();
    for seg in entry.rel_path.split('/') {
        if !seg.is_empty() {
            unit_path.push(seg);
        }
    }
    if !unit_path.is_dir() {
        return Err(format!("单位目录不存在：{}", entry.rel_path));
    }
    let input = NoteInput {
        entry: &entry,
        stats: &entry.stats,
        unit_path: &unit_path,
    };
    Ok(note_text(&input))
}

/// 单位封面缩略图：返回 data URL（解码成功 → 320px JPEG 缩略，失败 → 原字节 + 魔数 mime）。
/// 无封面返回 None。
#[tauri::command]
pub fn cover_image(root: String, rel_path: String) -> Result<Option<String>, String> {
    fsops::validate_rel(&rel_path)?;
    let mut unit = Path::new(&root).to_path_buf();
    for seg in rel_path.split('/') {
        if !seg.is_empty() {
            unit.push(seg);
        }
    }
    if !unit.is_dir() {
        return Err(format!("单位目录不存在：{}", rel_path));
    }
    let found = match meta::cover_bytes(&unit) {
        Some(x) => x,
        None => return Ok(None),
    };
    Ok(Some(thumb_data_url(&found.0, found.1)))
}

fn thumb_data_url(mime: &str, bytes: Vec<u8>) -> String {
    match image::load_from_memory(&bytes) {
        Ok(img) => {
            let thumb = img.thumbnail(320, 320);
            let mut out = std::io::Cursor::new(Vec::new());
            match thumb.write_to(&mut out, image::ImageFormat::Jpeg) {
                Ok(()) => data_url("image/jpeg", &out.into_inner()),
                Err(_) => data_url(mime, &bytes),
            }
        }
        Err(_) => data_url(mime, &bytes),
    }
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// 字段级对比结果：现有说明文件是否存在 + 保序字段列表
#[derive(Debug, Clone, Serialize)]
pub struct NoteDiff {
    pub existing: bool,
    pub fields: Vec<meta::FieldDiff>,
}

/// 字段级对比：现有 `<单位名>.txt` vs 按当前条目重新渲染的说明文件。
/// 字段以新渲染为主序（模板顺序），旧文件独有字段追加在末尾。
#[tauri::command(async)]
pub fn note_diff(root: String, entry: Entry) -> Result<NoteDiff, String> {
    fsops::validate_rel(&entry.rel_path)?;
    let mut unit_path = Path::new(&root).to_path_buf();
    for seg in entry.rel_path.split('/') {
        if !seg.is_empty() {
            unit_path.push(seg);
        }
    }
    if !unit_path.is_dir() {
        return Err(format!("单位目录不存在：{}", entry.rel_path));
    }
    let note_path = unit_path.join(format!("{}.txt", entry.name));
    let existing = std::fs::read(&note_path)
        .ok()
        .map(|b| meta::decode_bytes(&b));
    let input = NoteInput {
        entry: &entry,
        stats: &entry.stats,
        unit_path: &unit_path,
    };
    let rendered = note_text(&input);
    let had_existing = existing.is_some();
    let old = existing.unwrap_or_default();
    Ok(NoteDiff {
        existing: had_existing,
        fields: meta::diff_note_text(&old, &rendered),
    })
}

// ---------------------------------------------------------------- 说明文件批量核对

#[derive(Debug, Clone, Serialize)]
pub struct NoteAuditItem {
    pub rel_path: String,
    pub name: String,
    /// 说明文件是否存在
    pub exists: bool,
    /// 现有文件缺失的字段标签（新模板要求）
    pub missing: Vec<String>,
    pub changed: usize,
    pub added: usize,
    pub removed: usize,
    pub same: usize,
    /// 单条失败原因（如单位目录不存在）
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditProgress {
    /// unit | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rel_path: String,
    /// running | error | cancelled | finished
    pub status: String,
    pub message: String,
    pub item: Option<NoteAuditItem>,
}

/// 只读核对单个条目的说明文件（不写盘）
fn audit_one(root: &Path, entry: &Entry) -> NoteAuditItem {
    let mut item = NoteAuditItem {
        rel_path: entry.rel_path.clone(),
        name: entry.name.clone(),
        exists: false,
        missing: Vec::new(),
        changed: 0,
        added: 0,
        removed: 0,
        same: 0,
        error: None,
    };
    let mut unit_path = root.to_path_buf();
    for seg in entry.rel_path.split('/') {
        if !seg.is_empty() {
            unit_path.push(seg);
        }
    }
    if !unit_path.is_dir() {
        item.error = Some("单位目录不存在".into());
        return item;
    }
    let existing = std::fs::read(unit_path.join(format!("{}.txt", entry.name)))
        .ok()
        .map(|b| meta::decode_bytes(&b));
    let input = NoteInput {
        entry,
        stats: &entry.stats,
        unit_path: &unit_path,
    };
    let rendered = note_text(&input);
    let a = meta::audit_note(existing.as_deref(), &rendered);
    item.exists = a.exists;
    item.missing = a.missing;
    item.changed = a.changed;
    item.added = a.added;
    item.removed = a.removed;
    item.same = a.same;
    item
}

fn audit_worker(app: AppHandle, root: String, entries: Vec<Entry>, cancel: Arc<AtomicBool>) {
    let root_path = Path::new(&root);
    let total = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            let _ = app.emit(
                "note-audit-progress",
                AuditProgress {
                    phase: "finished".into(),
                    index: i,
                    total,
                    rel_path: entry.rel_path.clone(),
                    status: "cancelled".into(),
                    message: "已取消".into(),
                    item: None,
                },
            );
            return;
        }
        let item = audit_one(root_path, entry);
        let status = if item.error.is_some() { "error" } else { "running" };
        let message = item.error.clone().unwrap_or_default();
        let _ = app.emit(
            "note-audit-progress",
            AuditProgress {
                phase: "unit".into(),
                index: i,
                total,
                rel_path: entry.rel_path.clone(),
                status: status.into(),
                message,
                item: Some(item),
            },
        );
    }
    let _ = app.emit(
        "note-audit-progress",
        AuditProgress {
            phase: "finished".into(),
            index: total,
            total,
            rel_path: String::new(),
            status: "finished".into(),
            message: String::new(),
            item: None,
        },
    );
}

/// 批量核对说明文件（只读；与生成/导出互斥，可取消）
#[tauri::command]
pub fn start_note_audit(
    app: AppHandle,
    state: State<'_, JobState>,
    root: String,
    entries: Vec<Entry>,
) -> Result<(), String> {
    for e in &entries {
        fsops::validate_rel(&e.rel_path)?;
    }
    let (cancel, slot) = begin_job(&state)?;
    let app2 = app.clone();

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            audit_worker(app, root, entries, cancel);
        }));
        if result.is_err() {
            let _ = app2.emit(
                "note-audit-progress",
                AuditProgress {
                    phase: "finished".into(),
                    index: 0,
                    total: 0,
                    rel_path: String::new(),
                    status: "error".into(),
                    message: "任务异常终止（内部错误），请重试".into(),
                    item: None,
                },
            );
            eprintln!("[audit] worker panic，任务状态已复位");
        }
        end_job(&slot);
    });

    Ok(())
}

// ---------------------------------------------------------------- 查找重复文件

#[derive(Debug, Clone, Serialize)]
pub struct DupesProgress {
    /// scan | hash | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rel_path: String,
    /// running | cancelled | finished | error
    pub status: String,
    pub message: String,
    /// 仅 finished 事件携带（取消时为已得结果）
    pub groups: Vec<crate::dupes::DupGroup>,
}

fn dupes_worker(app: AppHandle, root: String, cancel: Arc<AtomicBool>) {
    let root_path = Path::new(&root);
    let mut last_scan_emit = 0usize;
    let groups = crate::dupes::find_duplicates(root_path, &cancel, |phase, index, total, rel| {
        // 扫描阶段文件多，每 200 个回一次；哈希阶段每个候选都回（进度直接可算）
        if phase == "scan" && index < last_scan_emit + 200 {
            return;
        }
        if phase == "scan" {
            last_scan_emit = index;
        }
        let _ = app.emit(
            "dupes-progress",
            DupesProgress {
                phase: phase.into(),
                index,
                total,
                rel_path: rel.into(),
                status: "running".into(),
                message: String::new(),
                groups: Vec::new(),
            },
        );
    });
    let cancelled = cancel.load(Ordering::SeqCst);
    let _ = app.emit(
        "dupes-progress",
        DupesProgress {
            phase: "finished".into(),
            index: 0,
            total: 0,
            rel_path: String::new(),
            status: if cancelled { "cancelled" } else { "finished" }.into(),
            message: String::new(),
            groups,
        },
    );
}

/// 查找根目录下内容完全相同的文件（只读；与生成/导出互斥，可取消）
#[tauri::command]
pub fn start_find_duplicates(
    app: AppHandle,
    state: State<'_, JobState>,
    root: String,
) -> Result<(), String> {
    if !Path::new(&root).is_dir() {
        return Err(format!("根目录不存在：{}", root));
    }
    let (cancel, slot) = begin_job(&state)?;
    let app2 = app.clone();

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dupes_worker(app, root, cancel);
        }));
        if result.is_err() {
            let _ = app2.emit(
                "dupes-progress",
                DupesProgress {
                    phase: "finished".into(),
                    index: 0,
                    total: 0,
                    rel_path: String::new(),
                    status: "error".into(),
                    message: "任务异常终止（内部错误），请重试".into(),
                    groups: Vec::new(),
                },
            );
            eprintln!("[dupes] worker panic，任务状态已复位");
        }
        end_job(&slot);
    });

    Ok(())
}

// ---------------------------------------------------------------- 导出清单校验

#[derive(Debug, Clone, Serialize)]
pub struct VerifyProgress {
    /// check | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    /// 当前校验的压缩包（相对输出根）
    pub rel_path: String,
    /// running | finished | cancelled | error
    pub status: String,
    pub message: String,
    /// 仅 finished 事件携带（取消时为已检部分）
    pub report: crate::manifest::VerifyReport,
}

fn verify_worker(app: AppHandle, output_root: String, cancel: Arc<AtomicBool>) {
    let out = Path::new(&output_root).to_path_buf();
    let result = crate::manifest::verify(&out, &cancel, &|index, total, rel| {
        let _ = app.emit(
            "verify-progress",
            VerifyProgress {
                phase: "check".into(),
                index,
                total,
                rel_path: rel.to_string(),
                status: "running".into(),
                message: String::new(),
                report: crate::manifest::VerifyReport::default(),
            },
        );
    });
    let cancelled = cancel.load(Ordering::SeqCst);
    let (status, message, report) = match result {
        Ok(r) => (
            if cancelled { "cancelled" } else { "finished" }.to_string(),
            String::new(),
            r,
        ),
        Err(e) => ("error".to_string(), e, crate::manifest::VerifyReport::default()),
    };
    let _ = app.emit(
        "verify-progress",
        VerifyProgress {
            phase: "finished".into(),
            index: report.items.len(),
            total: report.total,
            rel_path: String::new(),
            status,
            message,
            report,
        },
    );
}

/// 校验导出清单与磁盘现状（只读；与生成/导出互斥，可取消）
#[tauri::command]
pub fn start_verify(
    app: AppHandle,
    state: State<'_, JobState>,
    output_root: String,
) -> Result<(), String> {
    if output_root.trim().is_empty() {
        return Err("请先设置输出目录".into());
    }
    if !Path::new(&output_root).is_dir() {
        return Err(format!("输出目录不存在：{}", output_root));
    }
    let (cancel, slot) = begin_job(&state)?;
    let app2 = app.clone();

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verify_worker(app, output_root, cancel);
        }));
        if result.is_err() {
            let _ = app2.emit(
                "verify-progress",
                VerifyProgress {
                    phase: "finished".into(),
                    index: 0,
                    total: 0,
                    rel_path: String::new(),
                    status: "error".into(),
                    message: "任务异常终止（内部错误），请重试".into(),
                    report: crate::manifest::VerifyReport::default(),
                },
            );
            eprintln!("[verify] worker panic，任务状态已复位");
        }
        end_job(&slot);
    });

    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightIssue {
    /// error | warn | info
    pub level: String,
    pub rel_path: String,
    pub message: String,
}

/// 导出前自检：返回分级问题清单（error 阻碍成功，warn 仅提示）
#[tauri::command(async)]
pub fn preflight_export(
    root: String,
    output_root: String,
    entries: Vec<Entry>,
    settings: ArchiveSettings,
    profiles: Vec<PasswordProfile>,
) -> Result<Vec<PreflightIssue>, String> {
    let mut issues: Vec<PreflightIssue> = Vec::new();
    let mut push = |level: &str, rel: &str, message: String| {
        issues.push(PreflightIssue {
            level: level.to_string(),
            rel_path: rel.to_string(),
            message,
        })
    };

    if entries.is_empty() {
        push("error", "", "没有勾选任何条目".into());
    }

    // 1) 压缩工具
    if settings.format != "zip" && find_rar().is_none() {
        push("error", "", "未找到 WinRAR（Rar.exe），无法导出 RAR".into());
    }

    // 2) 输出根目录可用（临时文件探针）
    if output_root.trim().is_empty() {
        push("error", "", "未填写输出目录".into());
    } else {
        let out = Path::new(&output_root);
        let probe = out.join(".vinepress-write-probe.tmp");
        let ok = std::fs::create_dir_all(out)
            .and_then(|_| std::fs::write(&probe, b"ok"))
            .map(|_| {
                let _ = std::fs::remove_file(&probe);
            })
            .is_ok();
        if !ok {
            push("error", "", format!("输出目录不可写：{}", output_root));
        }
    }

    // 3) 注释与说明文件是否同步
    if settings.comment && !settings.write_notes {
        push(
            "warn",
            "",
            "已关闭「导出前生成说明文件」，压缩注释可能不是最新说明".into(),
        );
    }

    // 4) ZIP 的能力边界
    if settings.format == "zip" {
        if settings.volume_mib > 0 {
            push("warn", "", "ZIP 不支持分卷，将输出单个 .zip".into());
        }
        if settings.encrypt_names {
            push("warn", "", "ZIP 不支持加密，「加密文件名」会被忽略".into());
        }
    }

    let out_root = Path::new(&output_root);
    for e in &entries {
        if let Err(msg) = fsops::validate_rel(&e.rel_path) {
            push("error", &e.rel_path, msg);
            continue;
        }
        // 5) 单位目录存在性
        let mut unit = Path::new(&root).to_path_buf();
        for seg in e.rel_path.split('/') {
            if !seg.is_empty() {
                unit.push(seg);
            }
        }
        if !unit.is_dir() {
            push(
                "error",
                &e.rel_path,
                "单位目录不存在（可能已被移动或删除）".into(),
            );
        }

        // 6) 密码方案引用
        if let Some(pid) = &e.password_profile {
            if !profiles.iter().any(|p| &p.id == pid) {
                push("warn", &e.rel_path, "引用的密码方案已不存在".into());
            }
            if settings.format == "zip" {
                push("warn", &e.rel_path, "ZIP 不支持加密，密码方案会被忽略".into());
            }
        }

        // 7) 输出已存在且不覆盖 → 将跳过
        if !settings.overwrite && !output_root.trim().is_empty() {
            let mut dir = out_root.to_path_buf();
            for seg in e.rel_path.split('/') {
                if !seg.is_empty() {
                    dir.push(seg);
                }
            }
            let name = e.name.to_lowercase();
            if dir.is_dir()
                && std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.flatten().any(|f| {
                            let n = f.file_name().to_string_lossy().to_lowercase();
                            n.starts_with(&name) && (n.ends_with(".rar") || n.ends_with(".zip"))
                        })
                    })
                    .unwrap_or(false)
            {
                push(
                    "warn",
                    &e.rel_path,
                    "输出压缩包已存在且未勾选覆盖，将跳过".into(),
                );
            }
        }
    }

    // 8) 增量导出
    if settings.incremental {
        if output_root.trim().is_empty()
            || crate::manifest::load(Path::new(&output_root)).is_none()
        {
            push("info", "", "增量导出已开启，但尚无导出清单：本次将全量导出".into());
        } else {
            push("info", "", "增量导出已开启：源与设置未变动的单位将按清单跳过".into());
        }
    }

    Ok(issues)
}

/// 移入回收站（逐个校验根目录内，绝不永久删除）
#[tauri::command(async)]
pub fn trash_items(root: String, items: Vec<String>) -> Result<Vec<String>, String> {
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    for it in &items {
        let p = fsops::ensure_under(&root, it)?;
        if !p.exists() {
            return Err(format!("路径不存在：{}", it));
        }
        paths.push((it.clone(), p));
    }
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let targets: Vec<PathBuf> = paths.iter().map(|(_, p)| p.clone()).collect();
    trash::delete_all(&targets).map_err(|e| format!("移入回收站失败：{}", e))?;
    Ok(paths.into_iter().map(|(s, _)| s).collect())
}

/// 退出应用（菜单「文件 → 退出」；前端在任务中会先弹确认）
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn begin_job(state: &JobState) -> Result<(Arc<AtomicBool>, Arc<Mutex<Option<Arc<AtomicBool>>>>), String> {
    let slot = state.cancel.clone();
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return Err("已有任务正在运行".into());
    }
    let flag = Arc::new(AtomicBool::new(false));
    *guard = Some(flag.clone());
    drop(guard);
    Ok((flag, slot))
}

fn end_job(slot: &Arc<Mutex<Option<Arc<AtomicBool>>>>) {
    // 无论成败都必须复位，否则后续任务会永久卡在「已有任务正在运行」
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

fn find_rar() -> Option<String> {
    let candidates = [
        "C:\\Program Files\\WinRAR\\Rar.exe",
        "C:\\Program Files (x86)\\WinRAR\\Rar.exe",
        "D:\\Program Files\\WinRAR\\Rar.exe",
    ];
    for c in candidates {
        if Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    which("Rar.exe").or_else(|| which("rar.exe"))
}

fn find_seven_zip() -> Option<String> {
    let candidates = [
        "C:\\Program Files\\7-Zip\\7z.exe",
        "C:\\Program Files (x86)\\7-Zip\\7z.exe",
    ];
    for c in candidates {
        if Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    which("7z.exe")
}

fn which(cmd: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(cmd);
        if p.exists() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}
