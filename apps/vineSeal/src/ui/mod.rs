mod theme;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::core::conflict::ConflictPolicy;
use crate::core::hash::HashAlgo;
use crate::core::paths::available_space;
use crate::core::plan::build_plan;
use crate::core::transfer::{
    run_transfer, CopyOptions, FileOutcome, Progress, RunSummary, TransferMode, TransferOptions,
    VerifyMode,
};

pub fn run() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("vineSeal（藤印）")
            .with_inner_size([800.0, 560.0])
            .with_min_inner_size([640.0, 460.0]),
        ..Default::default()
    };
    match eframe::run_native(
        "vine-seal",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("启动界面失败：{e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Shared {
    progress: Progress,
    outcomes: Vec<FileOutcome>,
    summary: Option<RunSummary>,
    error: Option<String>,
}

struct App {
    src: String,
    dst: String,
    mode: TransferMode,
    algo: HashAlgo,
    perfect: bool,
    overwrite: bool,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
}

/// 中文字体：egui 默认字体不含 CJK，按优先级加载系统字体
/// （msyh.ttc / simhei.ttf / simsun.ttc / Deng.ttf 均已实测可被 ab_glyph 解析）
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

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        cc.egui_ctx.set_fonts(font_definitions());
        App {
            src: String::new(),
            dst: String::new(),
            mode: TransferMode::Copy,
            algo: HashAlgo::Sha256,
            perfect: false,
            overwrite: false,
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(Shared::default())),
        }
    }

    fn start(&mut self, ctx: egui::Context) {
        let src = PathBuf::from(self.src.trim());
        let dst = PathBuf::from(self.dst.trim());
        let mut shared = self.shared.lock().expect("shared lock");
        shared.error = None;
        shared.outcomes.clear();
        shared.summary = None;
        shared.progress = Progress::default();

        if self.src.trim().is_empty() {
            shared.error = Some("请选择源".into());
            return;
        }
        if self.dst.trim().is_empty() {
            shared.error = Some("请选择目标目录".into());
            return;
        }
        if !src.exists() {
            shared.error = Some(format!("源不存在：{}", src.display()));
            return;
        }
        if !dst.is_dir() {
            shared.error = Some(format!("目标目录不存在：{}", dst.display()));
            return;
        }
        let policy = if self.overwrite {
            ConflictPolicy::Overwrite
        } else {
            ConflictPolicy::Rename
        };
        let plan = match build_plan(&src, &dst, policy) {
            Ok(p) => p,
            Err(e) => {
                shared.error = Some(e.to_string());
                return;
            }
        };
        if plan.items.is_empty() {
            shared.error = Some("计划为空（没有可转移的文件）".into());
            return;
        }
        if let Ok(free) = available_space(&dst)
            && plan.transfer_bytes > free
        {
            shared.error = Some(format!(
                "目标空间不足：需要 {} 字节，可用 {free} 字节",
                plan.transfer_bytes
            ));
            return;
        }
        drop(shared);

        self.cancel.store(false, Ordering::Relaxed);
        self.running.store(true, Ordering::Relaxed);
        let opts = TransferOptions {
            mode: self.mode,
            copy: CopyOptions {
                algo: self.algo,
                verify: if self.perfect {
                    VerifyMode::Perfect
                } else {
                    VerifyMode::Conventional
                },
                ..Default::default()
            },
        };
        let cancel = self.cancel.clone();
        let running = self.running.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let summary = run_transfer(
                &plan,
                &opts,
                |p| {
                    shared.lock().expect("shared lock").progress = p.clone();
                    ctx.request_repaint();
                },
                &cancel,
            );
            let mut g = shared.lock().expect("shared lock");
            g.progress = Progress {
                files_done: summary.total,
                files_total: summary.total,
                bytes_done: summary.bytes,
                bytes_total: summary.bytes,
                current: String::new(),
            };
            g.outcomes = summary.outcomes.clone();
            g.summary = Some(summary);
            running.store(false, Ordering::Relaxed);
            drop(g);
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let running = self.running.load(Ordering::Relaxed);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("vineSeal")
                        .size(21.0)
                        .strong()
                        .color(theme::accent()),
                );
                ui.label(egui::RichText::new("（藤印）").size(21.0).color(theme::accent()));
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("复制 / 移动后哈希校验 · 校验不过绝不删源")
                        .color(theme::text_dim()),
                );
            });
            ui.add_space(6.0);

            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("源").strong());
                    let w = (ui.available_width() - 180.0).max(180.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.src)
                            .desired_width(w)
                            .hint_text("文件或文件夹路径"),
                    );
                    if ui.button("文件…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_file()
                    {
                        self.src = p.display().to_string();
                    }
                    if ui.button("文件夹…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_folder()
                    {
                        self.src = p.display().to_string();
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("目标").strong());
                    let w = (ui.available_width() - 110.0).max(180.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dst)
                            .desired_width(w)
                            .hint_text("目标目录"),
                    );
                    if ui.button("文件夹…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_folder()
                    {
                        self.dst = p.display().to_string();
                    }
                });
            });
            ui.add_space(8.0);

            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("模式").strong());
                    ui.selectable_value(&mut self.mode, TransferMode::Copy, "复制");
                    ui.selectable_value(&mut self.mode, TransferMode::Move, "移动");
                    ui.separator();
                    ui.label(egui::RichText::new("算法").strong());
                    for algo in HashAlgo::ALL {
                        ui.selectable_value(&mut self.algo, algo, algo.label());
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.perfect, "完美校验（复读源）");
                    ui.add_space(16.0);
                    ui.checkbox(&mut self.overwrite, "覆盖同名");
                });
            });
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let start = ui.add_enabled(
                    !running,
                    egui::Button::new(
                        egui::RichText::new("开始").strong().color(egui::Color32::WHITE),
                    )
                    .fill(theme::accent())
                    .min_size(egui::vec2(104.0, 30.0)),
                );
                if start.clicked() {
                    self.start(ui.ctx().clone());
                }
                let cancel = ui.add_enabled(
                    running,
                    egui::Button::new("取消").min_size(egui::vec2(76.0, 30.0)),
                );
                if cancel.clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
                if running {
                    ui.spinner();
                    ui.label(egui::RichText::new("进行中…").color(theme::text_dim()));
                }
            });

            let shared = self.shared.lock().expect("shared lock");

            let frac = if shared.progress.bytes_total > 0 {
                shared.progress.bytes_done as f32 / shared.progress.bytes_total as f32
            } else if shared.progress.files_total > 0 {
                shared.progress.files_done as f32 / shared.progress.files_total as f32
            } else {
                0.0
            };
            ui.add_space(6.0);
            ui.add(
                egui::ProgressBar::new(frac.clamp(0.0, 1.0))
                    .show_percentage()
                    .fill(theme::accent())
                    .corner_radius(egui::CornerRadius::same(6))
                    .desired_height(14.0),
            );
            if !shared.progress.current.is_empty() {
                // 当前文件：固定单行 + 超长截断，不挤压布局
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&shared.progress.current)
                            .monospace()
                            .color(theme::text_dim()),
                    )
                    .truncate(),
                );
            }

            if let Some(err) = &shared.error {
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(58, 24, 24))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 40, 40)))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::from_rgb(248, 113, 113), err);
                    });
            }

            if let Some(s) = &shared.summary {
                ui.add_space(6.0);
                let label = format!(
                    "合计：{} 成功 / {} 失败 / {} 跳过 / {} 已取消 · 传输 {}",
                    s.ok,
                    s.bad,
                    s.skipped,
                    s.cancelled,
                    fmt_size(s.bytes)
                );
                let color = if s.bad > 0 {
                    egui::Color32::from_rgb(248, 113, 113)
                } else {
                    egui::Color32::from_rgb(74, 222, 128)
                };
                ui.label(egui::RichText::new(label).strong().color(color));
            }

            if !shared.outcomes.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("结果明细")
                        .strong()
                        .color(theme::text_dim()),
                );
                ui.add_space(2.0);
                // 固定列宽：文件名/信息超长时省略号截断（悬停可见全文），不再把后续列挤走
                const COL_FILE: f32 = 300.0;
                const COL_SIZE: f32 = 84.0;
                const COL_MSG: f32 = 240.0;
                let row_h = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(260.0)
                    .show(ui, |ui| {
                        egui::Grid::new("outcomes")
                            .striped(true)
                            .num_columns(4)
                            .spacing([16.0, 6.0])
                            .min_col_width(44.0)
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("状态")
                                        .strong()
                                        .color(theme::text_dim()),
                                );
                                ui.label(
                                    egui::RichText::new("文件")
                                        .strong()
                                        .color(theme::text_dim()),
                                );
                                ui.add_sized(
                                    [COL_SIZE, row_h],
                                    egui::Label::new(
                                        egui::RichText::new("大小")
                                            .strong()
                                            .color(theme::text_dim()),
                                    )
                                    .halign(egui::Align::RIGHT),
                                );
                                ui.label(
                                    egui::RichText::new("信息")
                                        .strong()
                                        .color(theme::text_dim()),
                                );
                                ui.end_row();
                                for o in &shared.outcomes {
                                    ui.colored_label(theme::status_color(o.status), o.status.label());
                                    ui.add_sized(
                                        [COL_FILE, row_h],
                                        egui::Label::new(&o.rel).truncate(),
                                    )
                                    .on_hover_text(&o.rel);
                                    ui.add_sized(
                                        [COL_SIZE, row_h],
                                        egui::Label::new(
                                            egui::RichText::new(fmt_size(o.size)).monospace(),
                                        )
                                        .halign(egui::Align::RIGHT),
                                    );
                                    let msg = ui.add_sized(
                                        [COL_MSG, row_h],
                                        egui::Label::new(&o.message).truncate(),
                                    );
                                    if !o.message.is_empty() {
                                        msg.on_hover_text(&o.message);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
        });
    }
}

fn fmt_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
