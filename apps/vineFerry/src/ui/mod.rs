mod actions;
mod clip;
mod i18n;
mod jobs;
mod pane;
mod preview;
mod shell;
mod shell_icons;
mod theme;
mod thumbs;
mod transfer;
mod tree;

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::core::copy::{RunSummary, TransferMode};
use crate::core::hash::HashAlgo;
use crate::core::report::{summary_to_csv, write_report};
use crate::core::storage::{HistoryDb, RunRecord, VerifyRunRecord};
use jobs::JobHandle;
use pane::{PaneEvent, PaneGroup};
use transfer::TransferHandle;
use tree::NavTree;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\Deng.ttf",
    ];
    for p in candidates {
        if let Ok(bytes) = std::fs::read(p) {
            fonts
                .font_data
                .insert("cjk".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.push("cjk".into());
                }
            }
            break;
        }
    }
    fonts
}

fn format_ts(ts: i64) -> String {
    let (y, mo, d, h, mi) = civil_from_unix(ts);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

fn civil_from_unix(ts: i64) -> (i64, u32, u32, u32, u32) {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        ((secs % 3600) / 60) as u32,
    )
}

fn format_size_u64(n: u64) -> String {
    pane::format_size(n)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    None,
    Rename,
    NewFolder,
    Properties,
}

struct CtxMenu {
    pane: usize,
    pos: egui::Pos2,
    target: pane::MenuTarget,
    opened: std::time::Instant,
}

enum CtxAct {
    Open(PathBuf),
    Reveal(PathBuf),
    Rename(PathBuf),
    Trash(Vec<PathBuf>),
    CopyItems(Vec<PathBuf>),
    CutItems(Vec<PathBuf>),
    CopyPaths(Vec<PathBuf>),
    CopyTo(Vec<PathBuf>),
    MoveTo(Vec<PathBuf>),
    SendToOther(Vec<PathBuf>),
    Properties(Vec<PathBuf>),
    SetView(pane::ViewMode),
    SetSort(pane::SortKey, bool),
    Refresh,
    NewFolder,
    NewText,
    Paste,
    SelectAll,
}

/// 属性对话框缓存：打开时计算一次（含文件夹递归大小），关闭时清空。
struct PropsInfo {
    title: String,
    location: String,
    kind: String,
    /// 递归文件数（单文件=1）
    files: u64,
    /// 递归子文件夹数（不含顶层文件夹自身）
    dirs: u64,
    total_size: u64,
    modified: Option<std::time::SystemTime>,
    /// 单选时的文件属性文本（只读/隐藏/系统/存档），多选为 None
    attrs: Option<String>,
    single: bool,
    single_is_dir: bool,
}

fn bump_time(cur: &mut Option<std::time::SystemTime>, t: Option<std::time::SystemTime>) {
    if let Some(t) = t
        && cur.is_none_or(|c| t > c)
    {
        *cur = Some(t);
    }
}

fn attrs_text(a: u32) -> Option<String> {
    let ro = a & 0x1 != 0;
    let hidden = a & 0x2 != 0;
    let sys = a & 0x4 != 0;
    let archive = a & 0x20 != 0;
    let mut v: Vec<&str> = Vec::new();
    if ro {
        v.push(i18n::t("只读", "Read-only"));
    }
    if hidden {
        v.push(i18n::t("隐藏", "Hidden"));
    }
    if sys {
        v.push(i18n::t("系统", "System"));
    }
    if archive {
        v.push(i18n::t("存档", "Archive"));
    }
    if v.is_empty() { None } else { Some(v.join("、")) }
}

/// 单选：核心字段 + 文件夹递归大小；多选：汇总（项目数/类型构成/递归合计）。
fn compute_props(paths: &[PathBuf]) -> PropsInfo {
    use std::os::windows::fs::MetadataExt as _;

    let mut info = PropsInfo {
        title: String::new(),
        location: String::new(),
        kind: String::new(),
        files: 0,
        dirs: 0,
        total_size: 0,
        modified: None,
        attrs: None,
        single: paths.len() == 1,
        single_is_dir: false,
    };
    let mut top_dirs = 0u64;
    let mut top_files = 0u64;
    let mut parents: Vec<PathBuf> = Vec::new();

    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(crate::core::paths::to_long_path(path)) else {
            continue;
        };
        bump_time(&mut info.modified, meta.modified().ok());
        parents.push(
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone()),
        );
        if meta.is_dir() {
            top_dirs += 1;
            if info.single {
                info.single_is_dir = true;
            }
            // 目录联接/符号链接不递归，避免重复计入目标内容
            if crate::core::paths::is_reparse_point(&meta) {
                continue;
            }
            let it = walkdir::WalkDir::new(crate::core::paths::to_long_path(path))
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    e.depth() == 0
                        || e.metadata()
                            .map(|m| !crate::core::paths::is_reparse_point(&m))
                            .unwrap_or(false)
                });
            for entry in it.flatten() {
                if entry.depth() == 0 {
                    continue;
                }
                let Ok(m) = entry.metadata() else {
                    continue;
                };
                bump_time(&mut info.modified, m.modified().ok());
                if m.is_dir() {
                    info.dirs += 1;
                } else {
                    info.files += 1;
                    info.total_size += m.len();
                }
            }
        } else {
            top_files += 1;
            info.files += 1;
            info.total_size += meta.len();
        }
    }

    if info.single {
        let path = &paths[0];
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        info.location = parents
            .first()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        info.kind = pane::type_label(&name, info.single_is_dir).to_string();
        if let Ok(meta) = std::fs::symlink_metadata(crate::core::paths::to_long_path(path)) {
            info.attrs = attrs_text(meta.file_attributes());
        }
        info.title = name;
    } else {
        info.title = if i18n::en() {
            format!("{} items", paths.len())
        } else {
            format!("{} 个项目", paths.len())
        };
        let same_parent = parents.iter().all(|p| Some(p) == parents.first());
        info.location = if same_parent {
            parents
                .first()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else if i18n::en() {
            "— (multiple locations)".to_string()
        } else {
            "—（多个位置）".to_string()
        };
        info.kind = if i18n::en() {
            format!("{top_dirs} folders, {top_files} files")
        } else {
            format!("{top_dirs} 个文件夹，{top_files} 个文件")
        };
    }
    info
}

pub struct App {
    panes: [PaneGroup; 2],
    dual: bool,
    sync_nav: bool,
    active: usize,
    tree: NavTree,
    clipboard: Option<(ClipboardMode, Vec<PathBuf>)>,
    transfer: Option<TransferHandle>,
    last_summary: Option<RunSummary>,
    message: Option<String>,
    dialog_kind: DialogKind,
    dialog_path: PathBuf,
    /// 属性对话框的目标（单选=1 个路径；多选=汇总；空=当前目录）。
    dialog_paths: Vec<PathBuf>,
    /// 属性打开时计算一次并缓存（含文件夹递归大小），关闭时清空。
    props: Option<PropsInfo>,
    dialog_name: String,
    db: Option<HistoryDb>,
    job: Option<JobHandle>,
    pending_meta: Option<(TransferMode, String, String, String)>,
    show_history: bool,
    show_transfer_win: bool,
    history_runs: Vec<RunRecord>,
    history_verify_runs: Vec<VerifyRunRecord>,
    selected_run: Option<i64>,
    selected_verify_run: Option<i64>,
    preview: preview::PreviewPane,
    after_action: actions::AfterAction,
    show_presets: bool,
    preset_name: String,
    preset_interval_secs: u64,
    presets_tick: std::time::Instant,
    focus_address: Option<usize>,
    shot: Option<String>,
    shot_sent: bool,
    shot_frames: u8,
    ctx_menu: Option<CtxMenu>,
    thumbs: thumbs::ThumbCache,
    folder_peek: thumbs::FolderPeek,
    shell_icons: shell_icons::ShellIcons,
    shot_menu_added: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_fonts(font_definitions());
        theme::apply(&cc.egui_ctx);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from(r"D:\"));
        let dual = std::env::var("VINEFERRY_DUAL").is_ok();
        let grid = std::env::var("VINEFERRY_GRID").is_ok();
        let mut panes = [PaneGroup::new(cwd.clone()), PaneGroup::new(cwd)];
        if grid {
            for g in &mut panes {
                g.active_mut().view_mode = pane::ViewMode::Grid;
            }
        }
        App {
            panes,
            dual,
            sync_nav: false,
            active: 0,
            tree: NavTree::new(),
            clipboard: None,
            transfer: None,
            last_summary: None,
            message: None,
            dialog_kind: DialogKind::None,
            dialog_path: PathBuf::new(),
            dialog_paths: Vec::new(),
            props: None,
            dialog_name: String::new(),
            db: HistoryDb::open_default().ok(),
            job: None,
            pending_meta: None,
            show_history: false,
            show_transfer_win: false,
            history_runs: Vec::new(),
            history_verify_runs: Vec::new(),
            selected_run: None,
            selected_verify_run: None,
            preview: preview::PreviewPane::new(),
            after_action: actions::AfterAction::None,
            show_presets: false,
            preset_name: String::new(),
            preset_interval_secs: 0,
            presets_tick: std::time::Instant::now(),
            focus_address: None,
            shot: std::env::var("VINEFERRY_SHOT").ok(),
            shot_sent: false,
            shot_frames: 0,
            ctx_menu: None,
            thumbs: thumbs::ThumbCache::new(),
            folder_peek: thumbs::FolderPeek::new(),
            shell_icons: shell_icons::ShellIcons::new(),
            shot_menu_added: false,
        }
    }

    fn refresh_all(&mut self) {
        self.panes[0].reload_all();
        self.panes[1].reload_all();
        self.tree.invalidate();
    }

    fn set_msg(&mut self, msg: impl Into<String>) {
        self.message = Some(msg.into());
    }

    fn start(&mut self, items: Vec<PathBuf>, dest: PathBuf, mode: TransferMode) {
        match transfer::start_transfer(items.clone(), dest.clone(), mode) {
            Ok(handle) => {
                let src = items
                    .first()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.pending_meta = Some((
                    mode,
                    handle.label.clone(),
                    src,
                    dest.to_string_lossy().to_string(),
                ));
                self.set_msg(format!("已开始：{}", handle.label));
                self.transfer = Some(handle);
            }
            Err(e) => self.set_msg(format!("无法开始转移：{e}")),
        }
    }

    fn persist_run(&mut self, summary: RunSummary) {
        let meta = self.pending_meta.take();
        if let (Some(db), Some((mode, label, src, dst))) = (self.db.as_ref(), meta) {
            let err = db.insert_run(&label, mode, &src, &dst, &summary).err();
            if let Some(e) = err {
                self.set_msg(format!("写入历史失败：{e}"));
            }
        }
        self.last_summary = Some(summary);
    }

    fn poll_transfer(&mut self) {
        let Some(handle) = &self.transfer else { return };
        let state = transfer::poll(handle);
        if !state.finished {
            return;
        }
        let label = state.label.clone();
        self.transfer = None;
        if let Some(s) = state.summary {
            self.set_msg(format!(
                "完成：{label} —— 成功 {} / 失败 {} / 跳过 {} / 取消 {}",
                s.ok, s.bad, s.skipped, s.cancelled
            ));
            self.persist_run(s);
            self.refresh_all();
            if self.after_action != actions::AfterAction::None {
                let action = self.after_action;
                match action.trigger() {
                    Ok(()) => self.set_msg(format!(
                        "{} 已触发（关机/休眠有 30 秒缓冲；如需取消：shutdown /a）",
                        action.label()
                    )),
                    Err(e) => self.set_msg(format!("完成后动作失败：{e}")),
                }
                self.after_action = actions::AfterAction::None;
            }
        } else if let Some(e) = state.error {
            // 预扫描/空间预检失败：未搬任何数据，不落库、不触发完成后动作
            self.pending_meta = None;
            self.after_action = actions::AfterAction::None;
            self.set_msg(format!("计划失败：{e}"));
        } else {
            // 扫描期取消：同样未搬数据
            self.pending_meta = None;
            self.after_action = actions::AfterAction::None;
            self.set_msg(format!("已取消：{label}"));
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if self.transfer.is_some() {
            self.set_msg("已有转移任务进行中，拖入已忽略");
            return;
        }
        let dest = self.panes[self.active].active().current.clone();
        let valid: Vec<PathBuf> = dropped
            .into_iter()
            .filter(|p| p.exists() && !p.starts_with(&dest))
            .collect();
        if valid.is_empty() {
            self.set_msg("拖入的项目均无效（不存在或位于目标目录内）");
            return;
        }
        self.start(valid, dest, TransferMode::Copy);
    }

    fn poll_job(&mut self) {
        let Some(handle) = &self.job else { return };
        let snap = jobs::poll(handle);
        if !snap.finished {
            return;
        }
        let kind_manifest = matches!(handle.kind, jobs::JobKind::Manifest);
        let label = handle.label.clone();
        let (manifest_written, verify_outcome, error) = jobs::take_result(handle);
        self.job = None;
        if let Some(e) = error {
            self.set_msg(format!("{label}：{e}"));
        } else if let Some(path) = manifest_written {
            self.set_msg(format!("清单已写入：{path}"));
        } else if let Some(out) = verify_outcome {
            let (ok, missing, mismatch, read_error) = crate::core::verify::summarize(&out.items);
            let algo = out.algo.label().to_string();
            if let Some(db) = self.db.as_ref() {
                let _ = db.insert_verify_run(&algo, &out.manifest_path, &out.base, &out.items);
            }
            self.set_msg(format!(
                "校验完成：一致 {ok} / 缺失 {missing} / 不一致 {mismatch} / 读取失败 {read_error}"
            ));
        }
        if kind_manifest {
            self.refresh_all();
        }
    }

    fn reload_history(&mut self) {
        let result = self.db.as_ref().map(|db| {
            let runs = db.list_runs(200);
            let verifies = db.list_verify_runs(200);
            (runs, verifies)
        });
        match result {
            Some((Ok(runs), Ok(verifies))) => {
                self.history_runs = runs;
                self.history_verify_runs = verifies;
            }
            Some((Err(e), _)) | Some((_, Err(e))) => {
                self.set_msg(format!("读取历史失败：{e}"));
            }
            None => self.set_msg("历史数据库不可用"),
        }
    }

    fn export_recheck_manifest(&mut self, run_id: i64) {
        let Some(db) = self.db.as_ref() else {
            self.set_msg("历史数据库不可用");
            return;
        };
        let Ok(items) = db.list_verify_items(run_id) else {
            self.set_msg("读取校验明细失败");
            return;
        };
        let base = self
            .history_verify_runs
            .iter()
            .find(|r| r.id == run_id)
            .map(|r| r.base.clone())
            .unwrap_or_default();
        let name = format!("vineferry-recheck-{run_id}.sha256");
        let saved = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("哈希清单", &["sha256"])
            .save_file();
        if let Some(path) = saved {
            match crate::core::verify::write_recheck_manifest(&items, &path) {
                Ok(n) => self.set_msg(format!(
                    "已导出 {n} 项复检清单到 {}（基目录 {base}）",
                    path.display()
                )),
                Err(e) => self.set_msg(format!("导出失败：{e}")),
            }
        }
    }

    fn run_history_window(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        let mut open = self.show_history;
        let mut pick_run: Option<i64> = None;
        let mut pick_verify: Option<i64> = None;
        egui::Window::new(i18n::t("历史记录", "History"))
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.heading(i18n::t("转移历史", "Transfer History"));
                egui::ScrollArea::vertical()
                    .id_salt("runs")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        egui_extras::TableBuilder::new(ui)
                            .striped(true)
                            .column(egui_extras::Column::auto().at_least(130.0))
                            .column(egui_extras::Column::auto().at_least(60.0))
                            .column(egui_extras::Column::remainder().at_least(160.0))
                            .column(egui_extras::Column::auto().at_least(80.0))
                            .header(20.0, |mut h| {
                                h.col(|ui| {
                                    ui.strong("时间");
                                });
                                h.col(|ui| {
                                    ui.strong("结果");
                                });
                                h.col(|ui| {
                                    ui.strong("任务");
                                });
                                h.col(|ui| {
                                    ui.strong("字节");
                                });
                            })
                            .body(|body| {
                                body.rows(20.0, self.history_runs.len(), |mut row| {
                                    let r = &self.history_runs[row.index()];
                                    let selected = self.selected_run == Some(r.id);
                                    row.col(|ui| {
                                        let t = format_ts(r.ts);
                                        if ui.selectable_label(selected, t).clicked() {
                                            pick_run = Some(r.id);
                                        }
                                    });
                                    row.col(|ui| {
                                        ui.label(format!("{}/{}", r.ok, r.total));
                                    });
                                    row.col(|ui| {
                                        ui.label(format!("[{}] {}", r.mode, r.label));
                                    });
                                    row.col(|ui| {
                                        ui.label(format_size_u64(r.bytes));
                                    });
                                });
                            });
                    });
                if let Some(id) = self.selected_run {
                    ui.separator();
                    ui.heading(format!("任务 #{id} 明细"));
                    if let Some(db) = self.db.as_ref() {
                        match db.list_outcomes(id) {
                            Ok(items) => {
                                egui::ScrollArea::vertical()
                                    .id_salt("outcomes")
                                    .max_height(160.0)
                                    .show(ui, |ui| {
                                        for o in &items {
                                            let mark = match o.status {
                                                crate::core::copy::FileStatus::Verified => "✅",
                                                crate::core::copy::FileStatus::Failed => "❌",
                                                crate::core::copy::FileStatus::Skipped => "⏭",
                                                crate::core::copy::FileStatus::Cancelled => "⛔",
                                            };
                                            ui.label(format!("{mark} {} — {}", o.rel, o.message));
                                        }
                                    });
                            }
                            Err(e) => {
                                ui.colored_label(egui::Color32::LIGHT_RED, format!("读取失败：{e}"));
                            }
                        }
                    }
                }
                ui.separator();
                ui.heading(i18n::t("校验历史", "Verify History"));
                egui::ScrollArea::vertical()
                    .id_salt("verify_runs")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        egui_extras::TableBuilder::new(ui)
                            .striped(true)
                            .column(egui_extras::Column::auto().at_least(130.0))
                            .column(egui_extras::Column::auto().at_least(80.0))
                            .column(egui_extras::Column::remainder().at_least(140.0))
                            .column(egui_extras::Column::auto().at_least(150.0))
                            .header(20.0, |mut h| {
                                h.col(|ui| {
                                    ui.strong("时间");
                                });
                                h.col(|ui| {
                                    ui.strong("算法");
                                });
                                h.col(|ui| {
                                    ui.strong("基目录");
                                });
                                h.col(|ui| {
                                    ui.strong("一致/缺失/不一致");
                                });
                            })
                            .body(|body| {
                                body.rows(20.0, self.history_verify_runs.len(), |mut row| {
                                    let r = &self.history_verify_runs[row.index()];
                                    let selected = self.selected_verify_run == Some(r.id);
                                    row.col(|ui| {
                                        let t = format_ts(r.ts);
                                        if ui.selectable_label(selected, t).clicked() {
                                            pick_verify = Some(r.id);
                                        }
                                    });
                                    row.col(|ui| {
                                        ui.label(&r.algo);
                                    });
                                    row.col(|ui| {
                                        ui.label(&r.base);
                                    });
                                    row.col(|ui| {
                                        ui.label(format!(
                                            "{}/{}/{}",
                                            r.ok, r.missing, r.mismatch
                                        ));
                                    });
                                });
                            });
                    });
                if let Some(id) = self.selected_verify_run {
                    ui.separator();
                    ui.heading(format!("校验 #{id} 明细"));
                    let mut export_recheck = false;
                    if let Some(db) = self.db.as_ref() {
                        match db.list_verify_items(id) {
                            Ok(items) => {
                                egui::ScrollArea::vertical()
                                    .id_salt("verify_items")
                                    .max_height(160.0)
                                    .show(ui, |ui| {
                                        for it in &items {
                                            let mark = match it.status {
                                                crate::core::verify::VerifyStatus::Ok => "✅",
                                                crate::core::verify::VerifyStatus::Missing => "❌",
                                                crate::core::verify::VerifyStatus::Mismatch => "⚠",
                                                crate::core::verify::VerifyStatus::ReadError => "⛔",
                                            };
                                            ui.label(format!("{mark} {} — {}", it.path, it.message));
                                        }
                                    });
                                let problems = items
                                    .iter()
                                    .filter(|i| i.status != crate::core::verify::VerifyStatus::Ok)
                                    .count();
                                if ui
                                    .add_enabled(problems > 0, egui::Button::new("导出复检清单…"))
                                    .on_hover_text("将不一致/缺失/读取失败项导出为 .sha256，可在 vinePulse 中导入复检")
                                    .clicked()
                                {
                                    export_recheck = true;
                                }
                            }
                            Err(e) => {
                                ui.colored_label(egui::Color32::LIGHT_RED, format!("读取失败：{e}"));
                            }
                        }
                    }
                    if export_recheck {
                        self.export_recheck_manifest(id);
                    }
                }
                if ui.button("刷新").clicked() {
                    self.reload_history();
                }
            });
        if let Some(id) = pick_run {
            self.selected_run = Some(id);
        }
        if let Some(id) = pick_verify {
            self.selected_verify_run = Some(id);
        }
        self.show_history = open;
    }

    fn run_presets_window(&mut self, ctx: &egui::Context) {
        if !self.show_presets {
            return;
        }
        let mut open = self.show_presets;
        let mut run_id: Option<i64> = None;
        let mut delete_id: Option<i64> = None;
        let mut toggle: Option<(i64, String, bool, u64)> = None;
        let mut set_dest: Option<(i64, String, String, TransferMode, u64, bool, String)> = None;
        egui::Window::new(i18n::t("转移预设", "Transfer Presets"))
            .open(&mut open)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label("把当前活动栏目录保存为预设；间隔 > 0 时到点自动复制（忙碌则跳过本轮）。");
                ui.horizontal(|ui| {
                    ui.label("名称：");
                    ui.text_edit_singleline(&mut self.preset_name);
                    ui.label("间隔(秒)：");
                    let mut iv = self.preset_interval_secs;
                    ui.add(egui::DragValue::new(&mut iv).range(0..=86_400));
                    self.preset_interval_secs = iv;
                    let can_save = !self.preset_name.trim().is_empty();
                    if ui
                        .add_enabled(can_save, egui::Button::new("保存当前目录为预设"))
                        .clicked()
                    {
                        let src = self.panes[self.active].active().current.to_string_lossy().to_string();
                        let name = self.preset_name.trim().to_string();
                        let interval = self.preset_interval_secs;
                        match self.db.as_ref() {
                            Some(db) => {
                                match db.upsert_preset(
                                    &name,
                                    &src,
                                    "",
                                    TransferMode::Copy,
                                    interval,
                                    true,
                                ) {
                                    Ok(()) => {
                                        self.set_msg(format!("预设「{name}」已保存"));
                                        self.preset_name.clear();
                                    }
                                    Err(e) => self.set_msg(format!("保存预设失败：{e}")),
                                }
                            }
                            None => self.set_msg("历史数据库不可用"),
                        }
                    }
                });
                ui.separator();
                let presets = self
                    .db
                    .as_ref()
                    .and_then(|db| db.list_presets().ok())
                    .unwrap_or_default();
                if presets.is_empty() {
                    ui.label(egui::RichText::new("暂无预设").weak());
                }
                for p in &presets {
                    ui.horizontal(|ui| {
                        let mut en = p.enabled;
                        if ui.checkbox(&mut en, "").changed() {
                            toggle = Some((p.id, p.name.clone(), en, p.interval_secs));
                        }
                        ui.label(format!(
                            "{}（{} → {}，间隔 {}s{}）",
                            p.name,
                            match p.mode {
                                TransferMode::Copy => "复制",
                                TransferMode::Move => "移动",
                            },
                            if p.dst.is_empty() { "(未设目标)" } else { &p.dst },
                            p.interval_secs,
                            if p.last_run > 0 {
                                format!("，上次 {}s 前", now_secs().saturating_sub(p.last_run))
                            } else {
                                "，未运行".into()
                            }
                        ));
                        if ui.button("立即运行").clicked() {
                            run_id = Some(p.id);
                        }
                        if ui.button("设为当前栏为目标").clicked() {
                            let dst = self.panes[self.active].active().current.to_string_lossy().to_string();
                            set_dest = Some((
                                p.id,
                                p.name.clone(),
                                p.src.clone(),
                                p.mode,
                                p.interval_secs,
                                p.enabled,
                                dst,
                            ));
                        }
                        if ui.button("删除").clicked() {
                            delete_id = Some(p.id);
                        }
                    });
                }
            });
        self.show_presets = open;
        if let Some((id, _name, en, _iv)) = toggle
            && let Some(db) = self.db.as_ref()
        {
            let _ = db.set_preset_enabled(id, en);
        }
        if let Some((_id, name, src, mode, iv, en, dst)) = set_dest
            && let Some(db) = self.db.as_ref()
        {
            match db.upsert_preset(&name, &src, &dst, mode, iv, en) {
                Ok(()) => self.set_msg(format!("预设「{name}」目标已设为 {dst}")),
                Err(e) => self.set_msg(format!("更新预设失败：{e}")),
            }
        }
        if let Some(id) = delete_id
            && let Some(db) = self.db.as_ref()
        {
            let _ = db.delete_preset(id);
        }
        if let Some(id) = run_id {
            self.run_preset_now(id);
        }
    }

    fn run_preset_now(&mut self, preset_id: i64) {
        let Some(db) = self.db.as_ref() else {
            self.set_msg("历史数据库不可用");
            return;
        };
        let Ok(presets) = db.list_presets() else {
            return;
        };
        let Some(p) = presets.into_iter().find(|p| p.id == preset_id) else {
            return;
        };
        if p.dst.is_empty() {
            self.set_msg(format!(
                "预设「{}」未设置目标目录，暂无法自动运行",
                p.name
            ));
            return;
        }
        let _ = db.touch_preset(preset_id);
        self.start(vec![PathBuf::from(&p.src)], PathBuf::from(&p.dst), p.mode);
    }

    fn check_presets_due(&mut self) {
        if self.presets_tick.elapsed().as_secs() < 5 {
            return;
        }
        self.presets_tick = std::time::Instant::now();
        if self.transfer.is_some() || self.job.is_some() {
            return;
        }
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let Ok(presets) = db.list_presets() else {
            return;
        };
        let now = now_secs();
        for p in presets {
            if !p.enabled || p.interval_secs == 0 || p.dst.is_empty() {
                continue;
            }
            if now.saturating_sub(p.last_run) >= p.interval_secs {
                let src_ok = std::fs::metadata(&p.src).map(|m| m.is_dir()).unwrap_or(false);
                if !src_ok {
                    continue;
                }
                let _ = db.touch_preset(p.id);
                self.set_msg(format!("预设「{}」到点自动运行", p.name));
                self.start(vec![PathBuf::from(&p.src)], PathBuf::from(&p.dst), p.mode);
                return;
            }
        }
    }

    fn export_manifest(&mut self) {
        if self.job.is_some() || self.transfer.is_some() {
            self.set_msg("已有任务进行中");
            return;
        }
        let Some(dir) = rfd::FileDialog::new().pick_folder() else { return };
        let Some(save_to) = rfd::FileDialog::new()
            .set_file_name("manifest.sha256")
            .add_filter("SHA256 清单", &["sha256"])
            .save_file()
        else {
            return;
        };
        let handle = jobs::start_manifest_build(dir, HashAlgo::Sha256, save_to);
        self.set_msg(format!("已开始：{}", handle.label));
        self.job = Some(handle);
    }

    fn verify_manifest(&mut self) {
        if self.job.is_some() || self.transfer.is_some() {
            self.set_msg("已有任务进行中");
            return;
        }
        let Some(manifest) = rfd::FileDialog::new()
            .add_filter("清单", &["sha256", "sfv", "txt"])
            .pick_file()
        else {
            return;
        };
        let base = manifest
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(r"D:\"));
        let Some(base) = rfd::FileDialog::new()
            .set_directory(&base)
            .pick_folder()
        else {
            return;
        };
        let handle = jobs::start_verify(manifest, base, HashAlgo::Sha256);
        self.set_msg(format!("已开始：{}", handle.label));
        self.job = Some(handle);
    }

    fn trash_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        match trash::delete_all(&paths) {
            Ok(_) => {
                self.set_msg(format!("已将 {} 项移入回收站", paths.len()));
                self.refresh_all();
            }
            Err(e) => self.set_msg(format!("删除失败：{e}")),
        }
    }

    fn clipboard_paste(&mut self) {
        if let Some((mode, paths)) = self.clipboard.clone() {
            let dest = self.panes[self.active].active().current.clone();
            let m = if matches!(mode, ClipboardMode::Cut) {
                TransferMode::Move
            } else {
                TransferMode::Copy
            };
            self.start(paths, dest, m);
            self.clipboard = None;
            return;
        }
        let sys = clip::get_paths();
        if !sys.is_empty() {
            let dest = self.panes[self.active].active().current.clone();
            self.start(sys, dest, TransferMode::Copy);
        }
    }

    fn action_copy_cut(&mut self, mode: ClipboardMode) {
        let paths = self.panes[self.active].active().selected_paths();
        if !paths.is_empty() {
            let verb = match mode {
                ClipboardMode::Copy => "复制",
                ClipboardMode::Cut => "剪切",
            };
            self.set_msg(format!("已{verb} {} 项", paths.len()));
            self.clipboard = Some((mode, paths.clone()));
            clip::set_paths(&paths);
        }
    }

    fn action_copy_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\r\n");
        clip::set_text(&text);
        self.set_msg(format!("已复制 {} 项路径", paths.len()));
    }

    fn action_open_selected(&mut self) {
        let paths = self.panes[self.active].active().selected_paths();
        if paths.len() != 1 {
            return;
        }
        let p = paths[0].clone();
        if p.is_dir() {
            self.panes[self.active].active_mut().navigate(p);
        } else if let Err(e) = shell::open_path(&p.to_string_lossy()) {
            self.set_msg(e);
        }
    }

    fn action_delete(&mut self) {
        let paths = self.panes[self.active].active().selected_paths();
        self.trash_paths(paths);
    }

    fn action_rename(&mut self) {
        let paths = self.panes[self.active].active().selected_paths();
        if paths.len() == 1 {
            let path = paths[0].clone();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            self.panes[self.active].active_mut().pending_rename = None;
            self.dialog_path = path;
            self.dialog_name = name;
            self.dialog_kind = DialogKind::Rename;
        }
    }

    fn action_new_folder(&mut self) {
        self.dialog_name = i18n::t("新建文件夹", "New Folder").into();
        self.dialog_kind = DialogKind::NewFolder;
    }

    fn action_new_text(&mut self) {
        let dir = self.panes[self.active].active().current.clone();
        let base = i18n::t("新建 文本文档", "New Text Document");
        let mut name = format!("{base}.txt");
        let mut n = 2;
        while dir.join(&name).exists() {
            name = format!("{base} ({n}).txt");
            n += 1;
        }
        match std::fs::write(crate::core::paths::to_long_path(&dir.join(&name)), "") {
            Ok(_) => {
                self.set_msg(format!("已创建 {name}"));
                self.refresh_all();
            }
            Err(e) => self.set_msg(format!("创建失败：{e}")),
        }
    }

    fn tool_button(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        hover: String,
        paint: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let enabled = ui.is_enabled();
        if resp.hovered() && enabled {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(4), theme::HOVER);
        }
        let color = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        paint(ui.painter(), rect, color);
        resp.on_hover_text(hover)
    }

    fn paint_copy_icon(p: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let stroke = egui::Stroke::new(1.4, color);
        let size = egui::vec2(13.0, 13.0);
        let back =
            egui::Rect::from_min_size(rect.min + egui::vec2(9.0, 8.0), size);
        let front =
            egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), size);
        p.rect_stroke(back, egui::CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
        p.rect_stroke(front, egui::CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
    }

    fn paint_paste_icon(p: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let stroke = egui::Stroke::new(1.4, color);
        let body = egui::Rect::from_center_size(
            rect.center() + egui::vec2(0.0, 2.0),
            egui::vec2(15.0, 17.0),
        );
        p.rect_stroke(body, egui::CornerRadius::same(3), stroke, egui::StrokeKind::Middle);
        let tab = egui::Rect::from_min_size(
            egui::pos2(body.center().x - 4.5, body.top() - 4.0),
            egui::vec2(9.0, 6.0),
        );
        p.rect_stroke(tab, egui::CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
        let y1 = body.top() + 7.0;
        let y2 = body.top() + 11.5;
        p.line_segment(
            [
                egui::pos2(body.left() + 4.0, y1),
                egui::pos2(body.right() - 4.0, y1),
            ],
            stroke,
        );
        p.line_segment(
            [
                egui::pos2(body.left() + 4.0, y2),
                egui::pos2(body.center().x + 2.0, y2),
            ],
            stroke,
        );
    }

    fn action_select_all(&mut self) {
        self.panes[self.active].active_mut().select_all();
    }

    fn handle_keys(&mut self, ui: &mut egui::Ui) {
        if self.dialog_kind != DialogKind::None || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let key = ui.ctx().input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            if i.key_pressed(egui::Key::Delete) {
                Some("delete")
            } else if i.key_pressed(egui::Key::F2) {
                Some("f2")
            } else if i.key_pressed(egui::Key::F5) {
                Some("f5")
            } else if ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::C) {
                Some("copy")
            } else if ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::X) {
                Some("cut")
            } else if ctrl && i.key_pressed(egui::Key::V) {
                Some("paste")
            } else if ctrl && i.modifiers.shift && i.key_pressed(egui::Key::D) {
                Some("dual")
            } else if ctrl && i.modifiers.shift && i.key_pressed(egui::Key::N) {
                Some("new-folder")
            } else if ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::L) {
                Some("ctrl-l")
            } else if ctrl && i.key_pressed(egui::Key::A) {
                Some("select-all")
            } else if !ctrl && i.key_pressed(egui::Key::Escape) {
                Some("escape")
            } else if !ctrl && i.key_pressed(egui::Key::Backspace) {
                Some("backspace")
            } else if !ctrl && !i.modifiers.alt && i.key_pressed(egui::Key::Enter) {
                Some("enter")
            } else if i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft) {
                Some("alt-left")
            } else if i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight) {
                Some("alt-right")
            } else if i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp) {
                Some("alt-up")
            } else {
                None
            }
        });
        match key {
            Some("delete") => self.action_delete(),
            Some("f2") => self.action_rename(),
            Some("f5") => {
                self.panes[self.active].reload_all();
                self.tree.invalidate();
            }
            Some("copy") => self.action_copy_cut(ClipboardMode::Copy),
            Some("cut") => self.action_copy_cut(ClipboardMode::Cut),
            Some("paste") => self.clipboard_paste(),
            Some("dual") => self.dual = !self.dual,
            Some("new-folder") => self.action_new_folder(),
            Some("ctrl-l") => self.focus_address = Some(self.active),
            Some("select-all") => self.action_select_all(),
            Some("backspace") => self.panes[self.active].active_mut().go_up(),
            Some("enter") => self.action_open_selected(),
            Some("escape") => {
                if self.ctx_menu.is_none() {
                    let p = self.panes[self.active].active_mut();
                    p.selected.clear();
                    p.anchor = None;
                }
            }
            Some("alt-left") => self.panes[self.active].active_mut().go_back(),
            Some("alt-right") => self.panes[self.active].active_mut().go_forward(),
            Some("alt-up") => self.panes[self.active].active_mut().go_up(),
            _ => {}
        }
    }

    fn run_dialog(&mut self, ctx: &egui::Context) {
        match self.dialog_kind {
            DialogKind::None => {}
            DialogKind::Rename => {
                let path = self.dialog_path.clone();
                let mut ok = false;
                let mut cancel = false;
                egui::Window::new(i18n::t("重命名", "Rename"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
                    .show(ctx, |ui| {
                        ui.label(format!("{} {}", i18n::t("将", "Rename"), path.display()));
                        let resp = ui.text_edit_singleline(&mut self.dialog_name);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            ok = true;
                        }
                        ui.horizontal(|ui| {
                            if ui.button(i18n::t("确定", "OK")).clicked() {
                                ok = true;
                            }
                            if ui.button(i18n::t("取消", "Cancel")).clicked() {
                                cancel = true;
                            }
                        });
                    });
                if ok {
                    let new_name = self.dialog_name.trim().to_string();
                    if !new_name.is_empty() {
                        let new_path = path.with_file_name(&new_name);
                        match std::fs::rename(
                            crate::core::paths::to_long_path(&path),
                            crate::core::paths::to_long_path(&new_path),
                        ) {
                            Ok(_) => {
                                self.set_msg(format!("已重命名为 {new_name}"));
                                self.refresh_all();
                            }
                            Err(e) => self.set_msg(format!("重命名失败：{e}")),
                        }
                    }
                    self.dialog_kind = DialogKind::None;
                } else if cancel {
                    self.dialog_kind = DialogKind::None;
                }
            }
            DialogKind::NewFolder => {
                let dir = self.panes[self.active].active().current.clone();
                let mut ok = false;
                let mut cancel = false;
                egui::Window::new(i18n::t("新建文件夹", "New Folder"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
                    .show(ctx, |ui| {
                        ui.label(format!("{} {}", i18n::t("位置：", "Location:"), dir.display()));
                        let resp = ui.text_edit_singleline(&mut self.dialog_name);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            ok = true;
                        }
                        ui.horizontal(|ui| {
                            if ui.button(i18n::t("创建", "Create")).clicked() {
                                ok = true;
                            }
                            if ui.button(i18n::t("取消", "Cancel")).clicked() {
                                cancel = true;
                            }
                        });
                    });
                if ok {
                    let new_name = self.dialog_name.trim().to_string();
                    if !new_name.is_empty() {
                        let target = dir.join(&new_name);
                        match std::fs::create_dir(crate::core::paths::to_long_path(&target)) {
                            Ok(_) => {
                                self.set_msg(format!("已创建文件夹 {new_name}"));
                                self.refresh_all();
                            }
                            Err(e) => self.set_msg(format!("创建失败：{e}")),
                        }
                    }
                    self.dialog_kind = DialogKind::None;
                } else if cancel {
                    self.dialog_kind = DialogKind::None;
                }
            }
            DialogKind::Properties => {
                let paths = self.dialog_paths.clone();
                if self.props.is_none() {
                    self.props = Some(compute_props(&paths));
                }
                let Some(info) = self.props.as_ref() else {
                    self.dialog_kind = DialogKind::None;
                    return;
                };
                let mut close = false;
                let recursive = (info.single && info.single_is_dir) || !info.single;
                egui::Window::new(i18n::t("属性", "Properties"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
                    .show(ctx, |ui| {
                        egui::Grid::new("props")
                            .num_columns(2)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                ui.strong(i18n::t("名称", "Name"));
                                ui.label(&info.title);
                                ui.end_row();
                                ui.label(i18n::t("位置", "Location"));
                                ui.label(&info.location);
                                ui.end_row();
                                ui.label(i18n::t("类型", "Type"));
                                ui.label(&info.kind);
                                ui.end_row();
                                if !(info.single && !info.single_is_dir) {
                                    ui.label(i18n::t("包含", "Contains"));
                                    if i18n::en() {
                                        ui.label(format!(
                                            "{} files, {} folders",
                                            info.files, info.dirs
                                        ));
                                    } else {
                                        ui.label(format!(
                                            "{} 个文件，{} 个文件夹",
                                            info.files, info.dirs
                                        ));
                                    }
                                    ui.end_row();
                                }
                                ui.label(if recursive {
                                    i18n::t("大小（递归）", "Size (recursive)")
                                } else {
                                    i18n::t("大小", "Size")
                                });
                                ui.label(pane::format_size(info.total_size));
                                ui.end_row();
                                ui.label(i18n::t("修改时间", "Modified"));
                                ui.label(pane::format_time(info.modified));
                                ui.end_row();
                                ui.label(i18n::t("属性", "Attributes"));
                                ui.label(info.attrs.clone().unwrap_or_else(|| "—".into()));
                                ui.end_row();
                            });
                        if ui.button(i18n::t("关闭", "Close")).clicked() {
                            close = true;
                        }
                    });
                if close {
                    self.dialog_kind = DialogKind::None;
                    self.props = None;
                    self.dialog_paths.clear();
                }
            }
        }
    }

    /// 浮动转移窗口：列出活动转移（标签/进度/当前文件/取消）。
    /// 底部状态栏保留一行紧凑摘要，这里给完整视图。
    fn run_transfer_window(&mut self, ctx: &egui::Context) {
        let Some(state) = self.transfer.as_ref().map(transfer::poll) else {
            return;
        };
        let mut do_cancel = false;
        egui::Window::new(i18n::t("转移", "Transfer"))
            .open(&mut self.show_transfer_win)
            .default_width(400.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(&state.label);
                if state.scanning {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(i18n::t(
                            "正在计算计划与目标空间…",
                            "Building plan and checking space…",
                        ));
                    });
                } else if let Some(p) = &state.progress {
                    let frac = if p.files_total > 0 {
                        p.files_done as f32 / p.files_total as f32
                    } else {
                        0.0
                    };
                    ui.add(egui::ProgressBar::new(frac).show_percentage().desired_width(360.0));
                    ui.add(
                        egui::Label::new(format!(
                            "{}/{} {} · {}",
                            p.files_done,
                            p.files_total,
                            i18n::t("文件", "files"),
                            p.current
                        ))
                        .truncate(),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(i18n::t("取消", "Cancel")).clicked() {
                        do_cancel = true;
                    }
                });
            });
        if do_cancel
            && let Some(handle) = &self.transfer
        {
            handle.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn export_csv(&mut self) {
        let Some(summary) = &self.last_summary else {
            self.set_msg("暂无最近结果（完成一次转移后可用）");
            return;
        };
        let csv = summary_to_csv(summary);
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("vineferry-report.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
        {
            match write_report(&path, &csv) {
                Ok(()) => self.set_msg(format!("已导出：{}", path.display())),
                Err(e) => self.set_msg(format!("导出失败：{e}")),
            }
        }
    }

    fn execute(&mut self, events: Vec<PaneEvent>) {
        for ev in events {
            match ev {
                PaneEvent::Activate(i) => {
                    if i < 2 {
                        self.active = i;
                    }
                }
                PaneEvent::Navigate(i, p) => {
                    if i < 2 {
                        self.panes[i].active_mut().navigate(p.clone());
                        if self.sync_nav && self.dual {
                            let other = 1 - i;
                            self.panes[other].active_mut().navigate(p);
                        }
                    }
                }
                PaneEvent::OpenFile(p) => {
                    if let Err(e) = shell::open_path(&p.to_string_lossy()) {
                        self.set_msg(e);
                    }
                }
                PaneEvent::EditAddress(i, addr) => {
                    let p = PathBuf::from(addr);
                    if p.is_dir() {
                        self.panes[i].active_mut().navigate(p.clone());
                        if self.sync_nav && self.dual {
                            let other = 1 - i;
                            self.panes[other].active_mut().navigate(p);
                        }
                    } else {
                        self.set_msg(format!("路径不存在或不是目录：{}", p.display()));
                    }
                }
                PaneEvent::Refresh(i) => {
                    if i < 2 {
                        self.panes[i].reload_all();
                        self.tree.invalidate();
                    }
                }
                PaneEvent::GoBack(i) => self.panes[i].active_mut().go_back(),
                PaneEvent::GoForward(i) => self.panes[i].active_mut().go_forward(),
                PaneEvent::GoUp(i) => {
                    self.panes[i].active_mut().go_up();
                    if self.sync_nav && self.dual {
                        let other = 1 - i;
                        self.panes[other].active_mut().go_up();
                    }
                }
                PaneEvent::PickFolder(i) => {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.panes[i].active_mut().navigate(dir);
                    }
                }
                PaneEvent::AddressFocus(i) => self.focus_address = Some(i),
                PaneEvent::AddressDone(i) => {
                    if self.focus_address == Some(i) {
                        self.focus_address = None;
                    }
                }
                PaneEvent::ContextMenu(i, pos, target) => {
                    if i < 2 {
                        self.active = i;
                        self.ctx_menu = Some(CtxMenu {
                            pane: i,
                            pos,
                            target,
                            opened: std::time::Instant::now(),
                        });
                    }
                }
                PaneEvent::DragDrop(_from, paths, dest) => {
                    self.start(paths, dest, TransferMode::Copy);
                }
                PaneEvent::Rename(i, old, new) => {
                    if i < 2 {
                        let current = self.panes[i].active().current.clone();
                        let src = current.join(&old);
                        let dst = current.join(&new);
                        match std::fs::rename(&src, &dst) {
                            Ok(()) => {
                                self.panes[i].active_mut().reload();
                                self.tree.invalidate();
                            }
                            Err(e) => self.set_msg(format!(
                                "{}：{e}",
                                i18n::t("重命名失败", "Rename failed")
                            )),
                        }
                    }
                }
            }
        }
    }

    fn run_ctx_action(&mut self, pane_idx: usize, act: CtxAct) {
        if pane_idx < 2 {
            self.active = pane_idx;
        }
        match act {
            CtxAct::Open(p) => {
                if p.is_dir() {
                    self.panes[pane_idx].active_mut().navigate(p);
                } else if let Err(e) = shell::open_path(&p.to_string_lossy()) {
                    self.set_msg(e);
                }
            }
            CtxAct::Reveal(p) => {
                if let Err(e) = shell::reveal_path(&p.to_string_lossy()) {
                    self.set_msg(e);
                }
            }
            CtxAct::Rename(p) => {
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.panes[pane_idx].active_mut().pending_rename = None;
                self.dialog_path = p;
                self.dialog_name = name;
                self.dialog_kind = DialogKind::Rename;
            }
            CtxAct::Trash(paths) => self.trash_paths(paths),
            CtxAct::CopyItems(paths) => {
                self.clipboard = Some((ClipboardMode::Copy, paths.clone()));
                clip::set_paths(&paths);
                self.set_msg(format!("已复制 {} 项", paths.len()));
            }
            CtxAct::CutItems(paths) => {
                self.clipboard = Some((ClipboardMode::Cut, paths.clone()));
                clip::set_paths(&paths);
                self.set_msg(format!("已剪切 {} 项", paths.len()));
            }
            CtxAct::CopyPaths(paths) => self.action_copy_paths(paths),
            CtxAct::CopyTo(paths) => {
                if let Some(dest) = rfd::FileDialog::new().pick_folder() {
                    self.start(paths, dest, TransferMode::Copy);
                }
            }
            CtxAct::MoveTo(paths) => {
                if let Some(dest) = rfd::FileDialog::new().pick_folder() {
                    self.start(paths, dest, TransferMode::Move);
                }
            }
            CtxAct::SendToOther(paths) => {
                let other = 1 - pane_idx;
                let dest = self.panes[other].active().current.clone();
                self.start(paths, dest, TransferMode::Copy);
            }
            CtxAct::Properties(paths) => {
                self.dialog_paths = if paths.is_empty() {
                    vec![self.panes[pane_idx].active().current.clone()]
                } else {
                    paths
                };
                self.props = Some(compute_props(&self.dialog_paths));
                self.dialog_kind = DialogKind::Properties;
            }
            CtxAct::SetView(m) => self.panes[pane_idx].active_mut().view_mode = m,
            CtxAct::SetSort(k, asc) => {
                let p = self.panes[pane_idx].active_mut();
                p.sort = k;
                p.sort_asc = asc;
                p.sort_entries();
                p.selected.clear();
                p.anchor = None;
            }
            CtxAct::Refresh => {
                self.panes[pane_idx].reload_all();
                self.tree.invalidate();
            }
            CtxAct::NewFolder => {
                self.active = pane_idx;
                self.action_new_folder();
            }
            CtxAct::NewText => {
                self.active = pane_idx;
                self.action_new_text();
            }
            CtxAct::Paste => self.clipboard_paste(),
            CtxAct::SelectAll => {
                self.active = pane_idx;
                self.action_select_all();
            }
        }
    }

    fn run_context_menu(&mut self, ctx: &egui::Context) {
        let Some(menu) = self.ctx_menu.take() else {
            return;
        };
        let menu_pane = menu.pane;
        let dual = self.dual;
        let pane_view = self.panes[menu.pane].active().view_mode;
        let pane_sort = self.panes[menu.pane].active().sort;
        let pane_asc = self.panes[menu.pane].active().sort_asc;
        let has_clipboard = self.clipboard.is_some();
        let mut act: Option<CtxAct> = None;
        let pos = ctx
            .input(|i| i.viewport().inner_rect)
            .map(|r| {
                egui::pos2(
                    menu.pos.x.clamp(r.min.x + 4.0, (r.max.x - 230.0).max(r.min.x + 4.0)),
                    menu.pos.y
                        .clamp(r.min.y + 4.0, (r.max.y - 320.0).max(r.min.y + 4.0)),
                )
            })
            .unwrap_or(menu.pos);
        let resp = egui::Area::new(egui::Id::new("vine_ctx_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .fade_in(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    egui::menu::menu_style(ui.style_mut());
                    ui.set_min_width(216.0);
                    match &menu.target {
                        pane::MenuTarget::Items(paths) => {
                            let single = paths.len() == 1;
                            if single
                                && ui.button(i18n::t("打开", "Open")).clicked()
                            {
                                act = Some(CtxAct::Open(paths[0].clone()));
                            }
                            if single
                                && ui
                                    .button(i18n::t(
                                        "在资源管理器中定位",
                                        "Reveal in Explorer",
                                    ))
                                    .clicked()
                            {
                                act = Some(CtxAct::Reveal(paths[0].clone()));
                            }
                            ui.separator();
                            if ui
                                .button(format!("{} (Ctrl+X)", i18n::t("剪切", "Cut")))
                                .clicked()
                            {
                                act = Some(CtxAct::CutItems(paths.clone()));
                            }
                            if ui
                                .button(format!("{} (Ctrl+C)", i18n::t("复制", "Copy")))
                                .clicked()
                            {
                                act = Some(CtxAct::CopyItems(paths.clone()));
                            }
                            ui.separator();
                            if single
                                && ui
                                    .button(format!("{} (F2)", i18n::t("重命名…", "Rename…")))
                                    .clicked()
                            {
                                act = Some(CtxAct::Rename(paths[0].clone()));
                            }
                            if ui
                                .button(format!("{} (Del)", i18n::t("删除", "Delete")))
                                .on_hover_text(i18n::t("移入回收站", "Move to Recycle Bin"))
                                .clicked()
                            {
                                act = Some(CtxAct::Trash(paths.clone()));
                            }
                            ui.separator();
                            if ui
                                .button(i18n::t("复制路径", "Copy Path"))
                                .clicked()
                            {
                                act = Some(CtxAct::CopyPaths(paths.clone()));
                            }
                            if ui
                                .button(i18n::t("复制到…", "Copy to…"))
                                .clicked()
                            {
                                act = Some(CtxAct::CopyTo(paths.clone()));
                            }
                            if ui
                                .button(i18n::t("移动到…", "Move to…"))
                                .clicked()
                            {
                                act = Some(CtxAct::MoveTo(paths.clone()));
                            }
                            if dual
                                && ui
                                    .button(i18n::t("发送到另一栏", "Send to Other Pane"))
                                    .clicked()
                            {
                                act = Some(CtxAct::SendToOther(paths.clone()));
                            }
                            ui.separator();
                            if ui.button(i18n::t("属性", "Properties")).clicked() {
                                act = Some(CtxAct::Properties(paths.clone()));
                            }
                        }
                        pane::MenuTarget::Background => {
                            let view_menu = ui.menu_button(
                                format!("{} ›", i18n::t("查看", "View")),
                                |ui| {
                                    for m in [pane::ViewMode::List, pane::ViewMode::Grid] {
                                        if ui
                                            .selectable_label(pane_view == m, m.label())
                                            .clicked()
                                        {
                                            act = Some(CtxAct::SetView(m));
                                            ui.close();
                                        }
                                    }
                                },
                            );
                            view_menu.response.on_hover_text(i18n::t(
                                "切换列表/图标视图",
                                "Switch details / icons view",
                            ));
                            let sort_menu = ui.menu_button(
                                format!("{} ›", i18n::t("排序方式", "Sort by")),
                                |ui| {
                                    for k in pane::SortKey::ALL {
                                        let mark = if pane_sort == k {
                                            if pane_asc { " ▲" } else { " ▼" }
                                        } else {
                                            ""
                                        };
                                        if ui
                                            .selectable_label(pane_sort == k, format!("{}{mark}", k.label()))
                                            .clicked()
                                        {
                                            let asc = if pane_sort == k { !pane_asc } else { true };
                                            act = Some(CtxAct::SetSort(k, asc));
                                            ui.close();
                                        }
                                    }
                                },
                            );
                            sort_menu.response.on_hover_text(i18n::t(
                                "名称/日期/类型/大小",
                                "Name / date / type / size",
                            ));
                            ui.separator();
                            if ui
                                .button(format!("{} (F5)", i18n::t("刷新", "Refresh")))
                                .clicked()
                            {
                                act = Some(CtxAct::Refresh);
                            }
                            ui.separator();
                            ui.menu_button(
                                format!("{} ›", i18n::t("新建", "New")),
                                |ui| {
                                    if ui
                                        .button(i18n::t("文件夹…", "Folder…"))
                                        .clicked()
                                    {
                                        act = Some(CtxAct::NewFolder);
                                        ui.close();
                                    }
                                    if ui
                                        .button(i18n::t("文本文档", "Text Document"))
                                        .clicked()
                                    {
                                        act = Some(CtxAct::NewText);
                                        ui.close();
                                    }
                                },
                            );
                            ui.separator();
                            if ui
                                .add_enabled(
                                    has_clipboard,
                                    egui::Button::new(format!(
                                        "{} (Ctrl+V)",
                                        i18n::t("粘贴", "Paste")
                                    )),
                                )
                                .clicked()
                            {
                                act = Some(CtxAct::Paste);
                            }
                            if ui
                                .button(format!("{} (Ctrl+A)", i18n::t("全选", "Select All")))
                                .clicked()
                            {
                                act = Some(CtxAct::SelectAll);
                            }
                            ui.separator();
                            if ui.button(i18n::t("属性", "Properties")).clicked() {
                                act = Some(CtxAct::Properties(Vec::new()));
                            }
                        }
                    }
                });
            })
            .response;
        let old_enough = menu.opened.elapsed() >= std::time::Duration::from_millis(160);
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let outside_press = old_enough
            && ctx.input(|i| i.pointer.any_pressed())
            && !resp.contains_pointer();
        if act.is_none() && !esc && !outside_press {
            self.ctx_menu = Some(menu);
        }
        if let Some(a) = act {
            self.run_ctx_action(menu_pane, a);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_transfer();
        self.poll_job();
        if let Some(path) = self.shot.clone() {
            ui.ctx().request_repaint();
            if self.shot_sent {
                let imgs: Vec<std::sync::Arc<egui::ColorImage>> = ui.ctx().input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Screenshot { image, .. } => Some(image.clone()),
                            _ => None,
                        })
                        .collect()
                });
                for img in imgs {
                    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                    let mut buf = Vec::with_capacity(img.pixels.len() * 4);
                    for px in &img.pixels {
                        buf.extend_from_slice(&px.to_array());
                    }
                    match image::save_buffer(&path, &buf, w, h, image::ColorType::Rgba8) {
                        Ok(()) => self.shot = None,
                        Err(e) => eprintln!("screenshot save failed: {e}"),
                    }
                }
            } else if self.shot_frames >= 4 {
                if let Ok(spec) = std::env::var("VINEFERRY_MENU")
                    && !self.shot_menu_added
                {
                    let mut parts = spec.split(',');
                    let x: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(400.0);
                    let y: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(300.0);
                    self.ctx_menu = Some(CtxMenu {
                        pane: self.active,
                        pos: egui::pos2(x, y),
                        target: pane::MenuTarget::Background,
                        opened: std::time::Instant::now(),
                    });
                    self.shot_menu_added = true;
                } else {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Screenshot(
                        egui::UserData::new(()),
                    ));
                    self.shot_sent = true;
                }
            } else {
                self.shot_frames += 1;
            }
        }
        self.handle_keys(ui);
        self.handle_drops(ui.ctx());
        self.thumbs.begin_frame();
        self.folder_peek.begin_frame();

        let mut events: Vec<PaneEvent> = Vec::new();
        let mut tab_events: Vec<pane::TabEvent> = Vec::new();
        let focus_address = self.focus_address;

        egui::containers::Panel::top("cmdbar")
            .exact_size(40.0)
            .frame(theme::bar_frame())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::flat_buttons(ui);
                    let new_menu = ui.menu_button(
                        format!("{} ▼", i18n::t("新建", "New")),
                        |ui| {
                            if ui.button(i18n::t("文件夹…", "Folder…")).clicked() {
                                self.action_new_folder();
                                ui.close();
                            }
                            if ui
                                .button(i18n::t("文本文档", "Text Document"))
                                .clicked()
                            {
                                self.action_new_text();
                                ui.close();
                            }
                        },
                    );
                    new_menu.response.on_hover_text(i18n::t(
                        "在当前目录中新建",
                        "Create in the current folder",
                    ));
                    ui.separator();
                    {
                        let p = self.panes[self.active].active();
                        let can_back = p.can_back();
                        let can_fwd = p.can_forward();
                        let can_up = p.can_up();
                        if ui
                            .add_enabled(can_back, egui::Button::new("←"))
                            .on_hover_text(i18n::t("后退 (Alt+←)", "Back (Alt+←)"))
                            .clicked()
                        {
                            self.panes[self.active].active_mut().go_back();
                        }
                        if ui
                            .add_enabled(can_fwd, egui::Button::new("→"))
                            .on_hover_text(i18n::t("前进 (Alt+→)", "Forward (Alt+→)"))
                            .clicked()
                        {
                            self.panes[self.active].active_mut().go_forward();
                        }
                        if ui
                            .add_enabled(can_up, egui::Button::new("↑"))
                            .on_hover_text(i18n::t("上一级 (Alt+↑)", "Up (Alt+↑)"))
                            .clicked()
                        {
                            self.panes[self.active].active_mut().go_up();
                        }
                        if ui
                            .button("⟳")
                            .on_hover_text(i18n::t("刷新 (F5)", "Refresh (F5)"))
                            .clicked()
                        {
                            self.panes[self.active].reload_all();
                        }
                    }
                    ui.separator();
                    if ui
                        .button(i18n::t("全选", "Select All"))
                        .on_hover_text("Ctrl+A")
                        .clicked()
                    {
                        self.action_select_all();
                    }
                    if ui
                        .button(i18n::t("属性", "Properties"))
                        .on_hover_text("Alt+Enter")
                        .clicked()
                    {
                        let paths = self.panes[self.active].active().selected_paths();
                        self.run_ctx_action(self.active, CtxAct::Properties(paths));
                    }
                    ui.separator();
                    let sel = self.panes[self.active].active().selected_paths();
                    let has_sel = !sel.is_empty();
                    let single = sel.len() == 1;
                    if ui
                        .add_enabled(has_sel, egui::Button::new("✂"))
                        .on_hover_text(format!("{} (Ctrl+X)", i18n::t("剪切", "Cut")))
                        .clicked()
                    {
                        self.action_copy_cut(ClipboardMode::Cut);
                    }
                    ui.add_enabled_ui(has_sel, |ui| {
                        if Self::tool_button(
                            ui,
                            egui::vec2(26.0, 24.0),
                            format!("{} (Ctrl+C)", i18n::t("复制", "Copy")),
                            Self::paint_copy_icon,
                        )
                        .clicked()
                        {
                            self.action_copy_cut(ClipboardMode::Copy);
                        }
                    });
                    ui.add_enabled_ui(self.clipboard.is_some(), |ui| {
                        if Self::tool_button(
                            ui,
                            egui::vec2(26.0, 24.0),
                            format!("{} (Ctrl+V)", i18n::t("粘贴", "Paste")),
                            Self::paint_paste_icon,
                        )
                        .clicked()
                        {
                            self.clipboard_paste();
                        }
                    });
                    if ui
                        .add_enabled(single, egui::Button::new("✏"))
                        .on_hover_text(format!("{} (F2)", i18n::t("重命名", "Rename")))
                        .clicked()
                    {
                        self.action_rename();
                    }
                    if ui
                        .add_enabled(has_sel, egui::Button::new("🗑"))
                        .on_hover_text(format!("{} (Delete)", i18n::t("删除", "Delete")))
                        .clicked()
                    {
                        self.action_delete();
                    }
                    ui.separator();
                    if ui
                        .button(if self.dual {
                            i18n::t("单栏", "Single")
                        } else {
                            i18n::t("双栏", "Dual")
                        })
                        .on_hover_text(i18n::t("Ctrl+Shift+D 切换", "Toggle with Ctrl+Shift+D"))
                        .clicked()
                    {
                        self.dual = !self.dual;
                    }
                    if self.dual {
                        ui.toggle_value(&mut self.sync_nav, i18n::t("同步导航", "Sync Nav"))
                            .on_hover_text(i18n::t(
                                "双栏时一侧跳转，另一侧跟随",
                                "Both panes follow navigation",
                            ));
                    }
                    ui.toggle_value(&mut self.preview.enabled, i18n::t("预览", "Preview"))
                        .on_hover_text(i18n::t(
                            "右侧只读预览窗格（图片/文本）",
                            "Read-only preview pane (image/text)",
                        ));
                    ui.separator();
                    {
                        let vm = self.panes[self.active].active().view_mode;
                        if ui
                            .selectable_label(
                                vm == pane::ViewMode::List,
                                i18n::t("列表", "Details"),
                            )
                            .on_hover_text(i18n::t("详细信息列表视图", "Details list view"))
                            .clicked()
                        {
                            self.panes[self.active].active_mut().view_mode = pane::ViewMode::List;
                        }
                        if ui
                            .selectable_label(
                                vm == pane::ViewMode::Grid,
                                i18n::t("图标", "Icons"),
                            )
                            .on_hover_text(i18n::t(
                                "大图标网格视图（图片显示缩略图）",
                                "Icon grid view (image thumbnails)",
                            ))
                            .clicked()
                        {
                            self.panes[self.active].active_mut().view_mode = pane::ViewMode::Grid;
                        }
                    }
                    if ui
                        .button(i18n::t("选择目录…", "Choose Folder…"))
                        .clicked()
                    {
                        events.push(PaneEvent::PickFolder(self.active));
                    }
                    ui.separator();
                    let menu = ui.menu_button(
                        format!(
                            "{}：{}",
                            i18n::t("完成后", "When done"),
                            self.after_action.label()
                        ),
                        |ui| {
                            for a in actions::AfterAction::ALL {
                                if ui
                                    .selectable_label(self.after_action == a, a.label())
                                    .clicked()
                                {
                                    self.after_action = a;
                                    ui.close();
                                }
                            }
                        },
                    );
                    menu.response.on_hover_text(i18n::t(
                        "转移全部完成后执行（关机/睡眠有 30 秒缓冲提示）",
                        "Runs after all transfers finish (30s grace for shutdown/sleep)",
                    ));
                    if ui
                        .button(i18n::t("导出结果 CSV", "Export CSV"))
                        .on_hover_text(i18n::t(
                            "导出最近一次转移的逐文件结果",
                            "Export per-file results of the last transfer",
                        ))
                        .clicked()
                    {
                        self.export_csv();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::flat_buttons(ui);
                        if ui.button(i18n::lang_label()).clicked() {
                            i18n::toggle();
                        }
                        let busy = self.transfer.is_some() || self.job.is_some();
                        let more = ui.menu_button("…", |ui| {
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new(i18n::t("导出清单…", "Export Manifest…")),
                                )
                                .on_disabled_hover_text(i18n::t(
                                    "为当前目录生成 .sha256 清单",
                                    "Generate a .sha256 manifest for the current folder",
                                ))
                                .clicked()
                            {
                                self.export_manifest();
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new(i18n::t(
                                        "按清单校验…",
                                        "Verify Manifest…",
                                    )),
                                )
                                .on_disabled_hover_text(i18n::t(
                                    "选择 .sha256/.sfv 清单对目录做事后校验",
                                    "Verify a folder against a .sha256/.sfv manifest",
                                ))
                                .clicked()
                            {
                                self.verify_manifest();
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(i18n::t("历史", "History")).clicked() {
                                self.show_history = true;
                                ui.close();
                            }
                            if ui.button(i18n::t("预设", "Presets")).clicked() {
                                self.show_presets = true;
                                ui.close();
                            }
                        });
                        more.response.on_hover_text(i18n::t("更多操作", "More actions"));
                    });
                });
            });

        egui::containers::Panel::bottom("status")
            .frame(theme::bar_frame())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let pane = self.panes[self.active].active();
                    let view = pane.view_indices();
                    let view_len = view.len();
                    let dirs = view.iter().filter(|&&i| pane.entries[i].is_dir).count();
                    let files = view_len - dirs;
                    let total: u64 = view
                        .iter()
                        .filter(|&&i| !pane.entries[i].is_dir)
                        .map(|&i| pane.entries[i].size)
                        .sum();
                    ui.label(format!(
                        "{}  ·  {} {} / {} {}  ·  {}",
                        pane.current.display(),
                        dirs,
                        i18n::t("文件夹", "folders"),
                        files,
                        i18n::t("文件", "files"),
                        pane::format_size(total)
                    ));
                    if !pane.selected.is_empty() {
                        let ssize: u64 = pane
                            .selected
                            .iter()
                            .filter_map(|i| pane.entries.get(*i))
                            .filter(|e| !e.is_dir)
                            .map(|e| e.size)
                            .sum();
                        ui.separator();
                        let n = pane.selected.len();
                        if i18n::en() {
                            ui.label(format!("{n} selected · {}", pane::format_size(ssize)));
                        } else {
                            ui.label(format!(
                                "已选 {n} 项 · {}",
                                pane::format_size(ssize)
                            ));
                        }
                    }
                    if !pane.filter.is_empty() {
                        ui.separator();
                        ui.label(format!(
                            "{}「{}」→ {}",
                            i18n::t("筛选", "Filter"),
                            pane.filter,
                            view_len
                        ));
                    }
                    if let Some((mode, paths)) = &self.clipboard {
                        ui.separator();
                        ui.label(format!(
                            "{}：{} {}（{}）",
                            i18n::t("剪贴板", "Clipboard"),
                            match mode {
                                ClipboardMode::Copy => i18n::t("复制", "Copy"),
                                ClipboardMode::Cut => i18n::t("剪切", "Cut"),
                            },
                            paths.len(),
                            i18n::t("Ctrl+V 粘贴", "Ctrl+V to paste")
                        ));
                    }
                    if let Some(msg) = &self.message {
                        ui.separator();
                        ui.label(msg);
                    }
                    if crate::core::paths::is_portable() {
                        ui.separator();
                        ui.label(egui::RichText::new("便携模式").weak());
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let vm = self.panes[self.active].active().view_mode;
                            if ui
                                .selectable_label(
                                    vm == pane::ViewMode::Grid,
                                    i18n::t("图标", "Icons"),
                                )
                                .on_hover_text(i18n::t("大图标视图", "Large icons view"))
                                .clicked()
                            {
                                self.panes[self.active].active_mut().view_mode =
                                    pane::ViewMode::Grid;
                            }
                            if ui
                                .selectable_label(
                                    vm == pane::ViewMode::List,
                                    i18n::t("列表", "Details"),
                                )
                                .on_hover_text(i18n::t("详细信息列表视图", "Details list view"))
                                .clicked()
                            {
                                self.panes[self.active].active_mut().view_mode =
                                    pane::ViewMode::List;
                            }
                        },
                    );
                });
                let mut open_details = false;
                let mut do_cancel = false;
                if let Some(handle) = &self.transfer {
                    let state = transfer::poll(handle);
                    ui.horizontal(|ui| {
                        if state.scanning {
                            ui.label(format!(
                                "⏳ {} · {}",
                                state.label,
                                i18n::t("计划扫描中…", "Scanning…")
                            ));
                        } else {
                            ui.label(format!("⏳ {}", state.label));
                        }
                        if let Some(p) = &state.progress {
                            let frac = if p.files_total > 0 {
                                p.files_done as f32 / p.files_total as f32
                            } else {
                                0.0
                            };
                            ui.add(
                                egui::ProgressBar::new(frac)
                                    .desired_width(160.0)
                                    .show_percentage(),
                            );
                            ui.add(
                                egui::Label::new(format!(
                                    "{}/{} {} · {}",
                                    p.files_done,
                                    p.files_total,
                                    i18n::t("文件", "files"),
                                    p.current
                                ))
                                .truncate(),
                            );
                        }
                        if ui.small_button(i18n::t("详情", "Details")).clicked() {
                            open_details = true;
                        }
                        if ui.small_button(i18n::t("取消", "Cancel")).clicked() {
                            do_cancel = true;
                        }
                    });
                }
                if open_details {
                    self.show_transfer_win = true;
                }
                if do_cancel
                    && let Some(handle) = &self.transfer
                {
                    handle.cancel.store(true, Ordering::Relaxed);
                }
                if let Some(handle) = &self.job {
                    ui.horizontal(|ui| {
                        ui.label(format!("⏳ {}", handle.label));
                        if let Some((done, total)) = jobs::poll(handle).progress {
                            let frac = if total > 0 {
                                done as f32 / total as f32
                            } else {
                                0.0
                            };
                            ui.add(egui::ProgressBar::new(frac).desired_width(220.0));
                            ui.label(format!("{done}/{total}"));
                        }
                    });
                }
            });

        egui::containers::Panel::left("nav")
            .exact_size(210.0)
            .resizable(false)
            .show(ui, |ui| {
                let current = self.panes[self.active].active().current.clone();
                events.extend(tree::render_tree(
                    ui,
                    &mut self.tree,
                    &current,
                    self.active,
                ));
            });

        if self.preview.enabled {
            let sel = self.panes[self.active].active().selected_paths();
            self.preview.update(
                ui.ctx(),
                if sel.len() == 1 {
                    Some(sel[0].as_path())
                } else {
                    None
                },
            );
            egui::containers::Panel::right("preview")
                .default_size(260.0)
                .resizable(true)
                .show(ui, |ui| {
                    self.preview.render(ui);
                });
        }

        egui::containers::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::WHITE).inner_margin(0))
            .show(ui, |ui| {
            if self.dual {
                let act = self.active;
                ui.columns(2, |cols| {
                    let (e0, t0) = pane::render_group(
                        &mut cols[0],
                        0,
                        &mut self.panes[0],
                        focus_address == Some(0),
                        &mut self.thumbs,
                        &mut self.folder_peek,
                        &mut self.shell_icons,
                        act == 0,
                    );
                    let (e1, t1) = pane::render_group(
                        &mut cols[1],
                        1,
                        &mut self.panes[1],
                        focus_address == Some(1),
                        &mut self.thumbs,
                        &mut self.folder_peek,
                        &mut self.shell_icons,
                        act == 1,
                    );
                    events.extend(e0);
                    events.extend(e1);
                    tab_events.extend(t0);
                    tab_events.extend(t1);
                    let r = cols[0].max_rect();
                    let mid_x = r.right();
                    cols[0].painter().vline(
                        mid_x,
                        r.top()..=r.bottom(),
                        egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
                    );
                });
            } else {
                let (e0, t0) = pane::render_group(
                    ui,
                    0,
                    &mut self.panes[0],
                    focus_address.is_some(),
                    &mut self.thumbs,
                    &mut self.folder_peek,
                    &mut self.shell_icons,
                    true,
                );
                events.extend(e0);
                tab_events.extend(t0);
            }
        });

        self.execute(events);
        for te in tab_events {
            match te {
                pane::TabEvent::Add(i) => {
                    let cur = self.panes[i].active().current.clone();
                    self.panes[i].add_tab(cur);
                }
                pane::TabEvent::Close(i, idx) => self.panes[i].close_tab(idx),
                pane::TabEvent::Switch(i, idx) => self.panes[i].active = idx,
            }
        }
        self.run_dialog(ui.ctx());
        self.run_history_window(ui.ctx());
        self.run_presets_window(ui.ctx());
        self.run_transfer_window(ui.ctx());
        self.run_context_menu(ui.ctx());
        pane::render_drag_ghost(ui.ctx());
        self.check_presets_due();
        let copied = ui.ctx().output_mut(|o| clip::take_copy_text(&mut o.commands));
        for t in copied {
            clip::set_text(&t);
        }
    }
}

#[cfg(test)]
mod props_tests {
    use super::*;

    #[test]
    fn props_folder_recursive_size() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("a.txt"), vec![7u8; 10]).unwrap();
        std::fs::write(sub.join("b.txt"), vec![7u8; 5]).unwrap();
        let info = compute_props(&[dir.path().to_path_buf()]);
        assert!(info.single);
        assert!(info.single_is_dir);
        assert_eq!(info.files, 2);
        assert_eq!(info.dirs, 1);
        assert_eq!(info.total_size, 15);
        assert!(info.modified.is_some());
    }

    #[test]
    fn props_single_file_is_not_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("only.txt");
        std::fs::write(&f, vec![3u8; 9]).unwrap();
        let info = compute_props(std::slice::from_ref(&f));
        assert!(info.single);
        assert!(!info.single_is_dir);
        assert_eq!(info.files, 1);
        assert_eq!(info.dirs, 0);
        assert_eq!(info.total_size, 9);
        assert_eq!(info.location, dir.path().to_string_lossy().to_string());
    }

    #[test]
    fn props_multi_summary_sums_all() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("s");
        std::fs::create_dir(&sub).unwrap();
        let f1 = dir.path().join("a.txt");
        std::fs::write(&f1, vec![1u8; 3]).unwrap();
        std::fs::write(sub.join("b.txt"), vec![1u8; 4]).unwrap();
        let info = compute_props(&[f1, sub.to_path_buf()]);
        assert!(!info.single);
        assert_eq!(info.files, 2);
        assert_eq!(info.total_size, 7);
        assert!(!info.location.is_empty());
        assert!(!info.kind.is_empty());
        assert!(info.attrs.is_none());
    }
}
