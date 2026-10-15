//! 扫描页：目标管理（拖拽/选择器/清单）、后台扫描协作（进度/取消）、结果表（过滤/排序）

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::core::hash::{self, ManifestEntry};
use crate::walker;
use crate::{Opts, Report, Severity};

use super::{detail_view, severity_color, shell};

enum Event {
    Progress {
        done: usize,
        total: usize,
        bytes: u64,
    },
    /// 单个有问题的文件（扫描中流式推送，只含 problem_count>0 的报告）
    Report(Report),
    Finished {
        reports: Vec<Report>,
        cancelled: bool,
        elapsed: Duration,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Problems,
    Ok,
    Warning,
    Corrupt,
    Unknown,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "全部",
            Filter::Problems => "仅有问题",
            Filter::Ok => "正常",
            Filter::Warning => "可疑",
            Filter::Corrupt => "损坏",
            Filter::Unknown => "未知",
        }
    }

    fn matches(self, r: &Report) -> bool {
        match self {
            Filter::All => true,
            Filter::Problems => r.problem_count() > 0,
            Filter::Ok => r.worst() == Severity::Ok,
            Filter::Warning => r.worst() == Severity::Warning,
            Filter::Corrupt => r.worst() == Severity::Corrupt,
            Filter::Unknown => r.worst() == Severity::Unknown,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Name,
    Type,
    Severity,
    Problems,
}

/// 行操作（在表体闭包外执行，避免与 self 借用冲突）
enum RowAction {
    Reveal(PathBuf),
    Open(PathBuf),
    Copy(PathBuf),
}

struct ManifestInfo {
    path: PathBuf,
    entries: Vec<ManifestEntry>,
}

pub struct ScanState {
    targets: Vec<PathBuf>,
    manifest: Option<ManifestInfo>,

    running: bool,
    cancel: Option<Arc<AtomicBool>>,
    rx: Option<Receiver<Event>>,
    done: usize,
    total: usize,
    reports: Vec<Report>,
    /// 扫描中流式收到的问题文件数（FlacValidator 式「失败 N」统计）
    failed: usize,
    /// 已扫描文件累计字节数（用于速度显示）
    bytes: u64,
    /// 速度采样环形缓冲：(时刻, 累计字节数)。
    ///
    /// 保留最近约 2 秒的样本，展示时在窗口上取斜率（总量差 / 时间差）。
    /// 比单点增量稳，又比"从扫描开始算总平均"响应得快。
    speed_samples: VecDeque<(Instant, u64)>,
    /// 本轮扫描真实起点（用于已用/ETA）
    scan_start: Option<Instant>,
    /// 最近一次行操作（定位/打开/复制）的结果提示
    last_action_msg: Option<String>,
    cancelled: bool,
    elapsed: Duration,
    just_finished: bool,
    collect_errors: Vec<String>,

    filter: Filter,
    search: String,
    sort_col: SortCol,
    sort_asc: bool,
    view: Vec<usize>,
    view_dirty: bool,
    /// 上次重算 view 的时间：扫描中每帧都会置 dirty，全量重排会拖垮 UI，
    /// 用最小间隔节流（交互时仍即时）。
    last_view_rebuild: Option<Instant>,
    selected: Option<usize>,
}

impl ScanState {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            manifest: None,
            running: false,
            cancel: None,
            rx: None,
            done: 0,
            total: 0,
            reports: Vec::new(),
            failed: 0,
            bytes: 0,
            speed_samples: VecDeque::new(),
            scan_start: None,
            last_action_msg: None,
            cancelled: false,
            elapsed: Duration::ZERO,
            just_finished: false,
            collect_errors: Vec::new(),
            // FlacValidator 心智：默认只看问题文件；正常文件用「全部」过滤器可见
            filter: Filter::Problems,
            search: String::new(),
            sort_col: SortCol::Name,
            sort_asc: true,
            view: Vec::new(),
            view_dirty: false,
            last_view_rebuild: None,
            selected: None,
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn progress(&self) -> (usize, usize) {
        (self.done, self.total)
    }

    pub fn reports(&self) -> &[Report] {
        &self.reports
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn elapsed(&self) -> Duration {
        // 扫描进行中用真实起点，结束后用 worker 回报的 elapsed
        match (self.running, self.scan_start) {
            (true, Some(start)) => start.elapsed(),
            _ => self.elapsed,
        }
    }

    /// 扫描中流式累计的问题文件数
    pub fn failed_count(&self) -> usize {
        self.failed
    }

    /// 记录一次速度采样（时刻 + 累计字节数）。
    ///
    /// 每收到 Progress 事件调用；由 poll_events 按 ≥100ms 节流，
    /// 因此事件再密集也不会把瞬时噪声写进缓冲。
    fn push_speed_sample(&mut self) {
        let now = Instant::now();
        if let Some(&(last, _)) = self.speed_samples.back()
            && now.duration_since(last) < Duration::from_millis(100)
        {
            return;
        }
        self.speed_samples.push_back((now, self.bytes));
        // 只留最近 3 秒，防止长扫描下缓冲无限增长
        let cutoff = now - Duration::from_secs(3);
        while let Some(&(t, _)) = self.speed_samples.front() {
            if t < cutoff && self.speed_samples.len() > 2 {
                self.speed_samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// 平滑后的扫描速度（字节/秒）。
    ///
    /// 在缓冲窗口上取斜率；窗口两端至少相隔 300ms 才可信，
    /// 否则回落到"从扫描开始算"的总平均，避免开局数字乱跳。
    pub fn smoothed_speed(&self) -> f64 {
        const MIN_WINDOW: Duration = Duration::from_millis(300);
        if let (Some(&(t0, b0)), Some(&(t1, b1))) =
            (self.speed_samples.front(), self.speed_samples.back())
        {
            let dt = t1.duration_since(t0);
            if dt >= MIN_WINDOW {
                return b1.saturating_sub(b0) as f64 / dt.as_secs_f64();
            }
        }
        let elapsed = self.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.bytes as f64 / elapsed
        } else {
            0.0
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    /// 消费“本轮扫描结束”事件（mod.rs 里刷新状态栏文案）
    pub fn finished_event(&mut self) -> bool {
        std::mem::take(&mut self.just_finished)
    }

    pub fn targets(&self) -> &[PathBuf] {
        &self.targets
    }

    pub fn targets_empty(&self) -> bool {
        self.targets.is_empty() && self.manifest.is_none()
    }

    pub fn add_targets(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            if !self.targets.contains(&p) {
                self.targets.push(p);
            }
        }
        self.view_dirty = true;
    }

    pub fn clear_targets(&mut self) {
        self.targets.clear();
    }

    pub fn has_manifest(&self) -> bool {
        self.manifest.is_some()
    }

    /// (清单文件名, 文件数)
    pub fn manifest_info(&self) -> (String, usize) {
        match &self.manifest {
            Some(m) => (
                m.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| m.path.display().to_string()),
                m.entries.len(),
            ),
            None => (String::new(), 0),
        }
    }

    pub fn clear_manifest(&mut self) {
        self.manifest = None;
    }

    /// 弹出文件选择器导入哈希清单。返回 Ok(true)=已导入 / Ok(false)=用户取消 / Err=解析失败
    pub fn import_manifest(&mut self) -> Result<bool, String> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("哈希清单", &["sha256", "md5", "sfv"])
            .pick_file()
        else {
            return Ok(false);
        };
        let entries = hash::parse_manifest(&path)?;
        self.manifest = Some(ManifestInfo { path, entries });
        Ok(true)
    }

    pub fn request_cancel(&mut self) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// 启动后台扫描（清单模式以清单为扫描目标；否则展开 targets 目录）
    pub fn start(&mut self, opts: &mut Opts) -> Result<(), String> {
        if self.running {
            return Err("已有扫描在进行中".to_string());
        }
        let files: Vec<(PathBuf, u64)> = if let Some(m) = &self.manifest {
            opts.manifest_index = Some(Arc::new(hash::index_manifest(&m.entries)));
            opts.sidecar_hash = false;
            // 清单不含大小；这里每个条目取一次（仅扫描启动时，非热路径）
            m.entries
                .iter()
                .map(|e| {
                    let size = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
                    (e.path.clone(), size)
                })
                .collect()
        } else {
            let (files, errors) = walker::collect_files(&self.targets);
            self.collect_errors = errors;
            files
        };
        if files.is_empty() {
            return Err("没有找到可扫描的文件".to_string());
        }

        self.reports.clear();
        self.view.clear();
        self.selected = None;
        self.done = 0;
        self.total = files.len();
        self.failed = 0;
        self.bytes = 0;
        self.speed_samples.clear();
        self.cancelled = false;
        self.elapsed = Duration::ZERO;
        self.just_finished = false;
        self.view_dirty = true;

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        self.scan_start = Some(Instant::now());

        let total = files.len();
        let opts = opts.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let reports = walker::scan(&files, &opts, &cancel, |d, _t, report| {
                // 流式推送问题文件（FlacValidator 模式：扫描中就涌现坏文件）
                if report.problem_count() > 0 {
                    let _ = tx.send(Event::Report(report.clone()));
                }
                let _ = tx.send(Event::Progress {
                    done: d,
                    total,
                    // 大小来自目录遍历，避免每个文件再 stat 一次
                    bytes: report.size,
                });
            });
            let cancelled = cancel.load(Ordering::Relaxed);
            let _ = tx.send(Event::Finished {
                reports,
                cancelled,
                elapsed: start.elapsed(),
            });
        });
        Ok(())
    }

    pub fn poll_events(&mut self, ctx: &egui::Context) {
        let mut finished = None;
        // 采样放到 rx 借用结束后做，避免与 push_speed_sample 的 &mut self 冲突
        let mut saw_progress = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(Event::Progress { done, total, bytes }) => {
                        self.done = done;
                        self.total = total;
                        self.bytes += bytes;
                        saw_progress = true;
                    }
                    Ok(Event::Report(report)) => {
                        self.failed += 1;
                        self.reports.push(report);
                        self.view_dirty = true;
                    }
                    Ok(ev @ Event::Finished { .. }) => {
                        if let Event::Finished {
                            reports,
                            cancelled,
                            elapsed,
                        } = ev
                        {
                            finished = Some((reports, cancelled, elapsed));
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        if saw_progress {
            self.push_speed_sample();
        }
        if let Some((reports, cancelled, elapsed)) = finished {
            self.reports = reports;
            self.cancelled = cancelled;
            self.elapsed = elapsed;
            self.running = false;
            self.rx = None;
            self.cancel = None;
            self.scan_start = None;
            self.just_finished = true;
            self.view_dirty = true;
        }
        // 扫描进行中定时重绘（增量涌现 + 进度/速度/ETA 实时刷新）
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn rebuild_view(&mut self) {
        let q = self.search.to_lowercase();
        let mut view: Vec<usize> = self
            .reports
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                self.filter.matches(r)
                    && (q.is_empty()
                        || r.path.to_string_lossy().to_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect();

        let col = self.sort_col;
        let asc = self.sort_asc;
        view.sort_by(|&a, &b| {
            let ord = cmp_reports(&self.reports[a], &self.reports[b], col);
            if asc { ord } else { ord.reverse() }
        });

        self.view = view;
        self.view_dirty = false;
        if let Some(sel) = self.selected
            && sel >= self.view.len()
        {
            self.selected = None;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, status: &str) {
        self.handle_drop(ui.ctx());
        // egui 0.36：面板按顺序 show 在同一个父 Ui 上；左→中→右
        self.targets_panel(ui);
        self.central_panel(ui, status);
        self.detail_panel(ui);
    }

    fn handle_drop(&mut self, ui_ctx: &egui::Context) {
        if self.running {
            return;
        }
        let dropped = ui_ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let mut paths = Vec::new();
        for d in dropped {
            paths.push(d.path().to_path_buf());
        }
        if !paths.is_empty() {
            let n = paths.len();
            self.add_targets(paths);
            ui_ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "vinePulse —— 已添加 {n} 个目标"
            )));
        }
    }

    fn targets_panel(&mut self, ui: &mut egui::Ui) {
        let mut remove: Option<usize> = None;
        let targets_len = self.targets.len();
        let has_manifest = self.manifest.is_some();
        let manifest: Option<(String, usize)> = self
            .manifest
            .as_ref()
            .map(|m| (m.path.display().to_string(), m.entries.len()));
        let collect_err_count = self.collect_errors.len();

        egui::containers::Panel::left("targets")
            .resizable(true)
            .default_size(230.0)
            // 长路径不再把面板顶宽、把右侧结果表挤出窗口（需全屏才恢复）；
            // 仅封顶自动增长，仍可手动拖动到 400（超出部分 hover 显示完整路径）。
            .max_size(400.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading("扫描目标");
                ui.add_space(2.0);
                if let Some((path, n)) = manifest {
                    ui.label("哈希清单模式");
                    ui.label(egui::RichText::new(path).small());
                    ui.label(format!("{n} 个文件（以清单为扫描目标并比对哈希）"));
                    ui.add_space(4.0);
                }
                if !has_manifest {
                    if targets_len == 0 {
                        ui.label(
                            egui::RichText::new("空：点击工具栏添加，或拖拽文件/文件夹到窗口")
                                .weak(),
                        );
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (i, p) in self.targets.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let name = display_path(p);
                                // 长名截断时给尾部 ✕ 预留空间：否则整行超出面板宽度，
                                // 既把 ✕ 挤出可视/可点区，又把面板顶宽、把结果表挤出窗口。
                                let button_w = ui.spacing().interact_size.x
                                    + ui.spacing().item_spacing.x;
                                let label_w = (ui.available_width() - button_w).max(40.0);
                                ui.add_sized(
                                    [label_w, ui.spacing().interact_size.y],
                                    egui::Label::new(&name)
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text(p.display().to_string());
                                if ui.small_button("✕").on_hover_text("移除").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                    });
                }
                if collect_err_count > 0 {
                    ui.separator();
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("{collect_err_count} 个路径无法读取"),
                    );
                }
            });
        if let Some(i) = remove
            && i < self.targets.len()
        {
            self.targets.remove(i);
        }
    }

    fn central_panel(&mut self, ui: &mut egui::Ui, status: &str) {
        if self.view_dirty {
            let now = Instant::now();
            let interactive = !self.running;
            let throttled = self
                .last_view_rebuild
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(150));
            // 扫描中限流（150ms 一次）；过滤/搜索/排序等交互操作始终立即生效
            if interactive || !throttled {
                self.rebuild_view();
                self.last_view_rebuild = Some(now);
            }
        }

        let mut clicked: Option<SortCol> = None;
        let mut filter_changed = false;

        egui::containers::CentralPanel::default().show(ui, |ui| {
            if self.reports.is_empty() && !self.running {
                ui.add_space(80.0);
                ui.vertical_centered(|ui| {
                    ui.heading("vinePulse 文件损坏检测");
                    ui.add_space(8.0);
                    ui.label(status);
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("支持拖拽文件/文件夹，或用工具栏添加与导入哈希清单")
                            .weak(),
                    );
                });
                return;
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("filter")
                    .selected_text(self.filter.label())
                    .show_ui(ui, |ui| {
                        for f in [
                            Filter::All,
                            Filter::Problems,
                            Filter::Ok,
                            Filter::Warning,
                            Filter::Corrupt,
                            Filter::Unknown,
                        ] {
                            if ui
                                .selectable_value(&mut self.filter, f, f.label())
                                .changed()
                            {
                                filter_changed = true;
                            }
                        }
                    });
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("搜索文件名/路径…")
                        .desired_width(220.0),
                );
                if resp.changed() {
                    filter_changed = true;
                }
                ui.label(
                    egui::RichText::new(format!(
                        "显示 {}/{}",
                        self.view.len(),
                        self.reports.len()
                    ))
                    .weak(),
                );
                if let Some(msg) = &self.last_action_msg
                    && !msg.is_empty()
                {
                    ui.separator();
                    ui.label(egui::RichText::new(msg).weak());
                }
            });
            ui.add_space(2.0);

            let ScanState {
                reports,
                view,
                selected,
                last_action_msg,
                ..
            } = self;

            let mut row_action: Option<RowAction> = None;

            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .column(Column::remainder().at_least(180.0))
                .column(Column::auto().at_least(70.0))
                .column(Column::auto().at_least(56.0))
                .column(Column::auto().at_least(56.0))
                .column(Column::auto().at_least(56.0))
                // 首因徽章列：自然宽度（徽章内部截断，不撑破布局）
                .column(Column::auto().at_least(160.0))
                .header(22.0, |mut header| {
                    header.col(|ui| {
                        header_cell(ui, "文件名", SortCol::Name, &mut clicked);
                    });
                    header.col(|ui| {
                        header_cell(ui, "类型", SortCol::Type, &mut clicked);
                    });
                    header.col(|ui| {
                        header_cell(ui, "结论", SortCol::Severity, &mut clicked);
                    });
                    header.col(|ui| {
                        header_cell(ui, "问题数", SortCol::Problems, &mut clicked);
                    });
                    header.col(|ui| {
                        header_cell(ui, "首因", SortCol::Problems, &mut clicked);
                    });
                })
                .body(|body| {
                    body.rows(20.0, view.len(), |mut row| {
                        let vi = row.index();
                        let idx = view[vi];
                        let r = &reports[idx];
                        let sev = r.worst();
                        let is_sel = *selected == Some(vi);
                        row.col(|ui| {
                            let name = r
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| r.path.display().to_string());
                            let resp = ui
                                .selectable_label(is_sel, name)
                                .on_hover_text(r.path.display().to_string());
                            if resp.clicked() {
                                *selected = Some(vi);
                            }
                        });
                        row.col(|ui| {
                            ui.label(&r.file_type);
                        });
                        row.col(|ui| {
                            ui.colored_label(severity_color(sev), sev.label());
                        });
                        row.col(|ui| {
                            let n = r.problem_count();
                            if n > 0 {
                                ui.colored_label(severity_color(sev), n.to_string());
                            } else {
                                ui.label("0");
                            }
                        });
                        row.col(|ui| {
                            render_finding_badge(ui, r, sev);
                        });

                        // 行级交互：双击定位 + 右键菜单
                        let row_resp = row.response();
                        if row_resp.double_clicked() {
                            row_action = Some(RowAction::Reveal(r.path.clone()));
                        }
                        row_resp.context_menu(|ui| {
                            if ui.button("在资源管理器中定位").clicked() {
                                row_action = Some(RowAction::Reveal(r.path.clone()));
                                ui.close();
                            }
                            if ui.button("打开文件").clicked() {
                                row_action = Some(RowAction::Open(r.path.clone()));
                                ui.close();
                            }
                            if ui.button("复制路径").clicked() {
                                row_action = Some(RowAction::Copy(r.path.clone()));
                                ui.close();
                            }
                        });
                    });
                });

            if let Some(action) = row_action {
                let result = match &action {
                    RowAction::Reveal(p) => shell::reveal_path(&p.to_string_lossy()),
                    RowAction::Open(p) => shell::open_path(&p.to_string_lossy()),
                    RowAction::Copy(p) => {
                        ui.ctx().copy_text(p.display().to_string());
                        Ok(())
                    }
                };
                *last_action_msg = Some(match result {
                    Ok(()) => match action {
                        RowAction::Copy(_) => "已复制路径到剪贴板".to_string(),
                        _ => String::new(),
                    },
                    Err(e) => e,
                });
            }
        });

        if let Some(col) = clicked {
            if self.sort_col == col {
                self.sort_asc = !self.sort_asc;
            } else {
                self.sort_col = col;
                self.sort_asc = true;
            }
            self.view_dirty = true;
        }
        if filter_changed {
            self.view_dirty = true;
        }
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        egui::containers::Panel::right("detail")
            .resizable(true)
            .default_size(340.0)
            .show(ui, |ui| {
                let report = self
                    .selected
                    .and_then(|vi| self.view.get(vi).copied())
                    .and_then(|idx| self.reports.get(idx));
                detail_view::ui(ui, report);
            });
    }
}

fn header_cell(
    ui: &mut egui::Ui,
    label: &str,
    col: SortCol,
    clicked: &mut Option<SortCol>,
) {
    let resp = ui.add(egui::Label::new(egui::RichText::new(label).strong()).sense(egui::Sense::click()));
    if resp.clicked() {
        *clicked = Some(col);
    }
}

fn cmp_reports(a: &Report, b: &Report, col: SortCol) -> std::cmp::Ordering {
    match col {
        SortCol::Name => file_name(a).cmp(&file_name(b)).then_with(|| a.path.cmp(&b.path)),
        SortCol::Type => a.file_type.cmp(&b.file_type).then_with(|| a.path.cmp(&b.path)),
        SortCol::Severity => a
            .worst()
            .rank()
            .cmp(&b.worst().rank())
            .then_with(|| a.path.cmp(&b.path)),
        SortCol::Problems => a
            .problem_count()
            .cmp(&b.problem_count())
            .then_with(|| a.path.cmp(&b.path)),
    }
}

fn file_name(r: &Report) -> String {
    r.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| r.path.display().to_string())
}

/// 首因徽章：只显示最高严重度问题的一行摘要（表格保持克制），
/// 完整原因列表挂在悬停提示上，点选行后在右侧详情面板展开。
fn render_finding_badge(ui: &mut egui::Ui, r: &Report, sev: Severity) {
    let Some(f) = r.primary_finding() else {
        ui.label(egui::RichText::new("—").weak());
        return;
    };

    // 徽章文本控制在一行内截断，避免长原因撑宽列/破坏行高
    let mut badge = egui::RichText::new(truncate_chars(&f.message, 28)).color(severity_color(sev));
    if f.offset.is_some() {
        badge = badge.strong();
    }
    let resp = ui
        .add(egui::Label::new(badge).truncate())
        .on_hover_ui(|ui| {
            // 悬停给出完整列表：code + severity + message + offset
            ui.set_max_width(420.0);
            ui.strong(format!("{} 个问题", r.problem_count()));
            ui.separator();
            for p in r.findings.iter().filter(|x| x.severity.is_problem()) {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(severity_color(p.severity), p.severity.label());
                    ui.monospace(p.code);
                });
                ui.label(&p.message);
                if let Some(off) = p.offset {
                    ui.label(
                        egui::RichText::new(format!("偏移 0x{off:X}"))
                            .weak()
                            .small(),
                    );
                }
                ui.add_space(4.0);
            }
        });
    // 悬停文本之外，再给一个纯文本兜底（无障碍/快速扫读）
    resp.on_hover_text(f.message.clone());
}

/// 按字符截断（中文安全，不切半个字符），超出加省略号
fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn display_path(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}
