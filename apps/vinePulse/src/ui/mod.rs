//! M3 图形界面：无参数启动 eframe 主窗口（带参数走 CLI，--rpc 最早分流）
//!
//! 模块布局遵循 README 架构：`mod.rs` = 应用壳/状态栏/页面切换，
//! `scan_view.rs` = 目标管理/拖拽/进度/取消/结果表，`detail_view.rs` = 详情与报告导出。

mod detail_view;
mod scan_view;
pub mod shell;

use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::Opts;
use crate::core::dispatch::Summary;
use crate::verifiers::tools;

/// 无参数启动图形界面（main.rs 入口分流调用）
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("vinePulse —— 文件损坏检测")
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([760.0, 480.0])
            .with_max_inner_size([1280.0, 860.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vinePulse",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_fonts(font_definitions());
            Ok(Box::new(PulseApp::new()))
        }),
    )
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

pub fn severity_color(sev: crate::Severity) -> egui::Color32 {
    match sev {
        crate::Severity::Ok => egui::Color32::from_rgb(80, 176, 80),
        crate::Severity::Warning => egui::Color32::from_rgb(218, 165, 32),
        crate::Severity::Corrupt => egui::Color32::from_rgb(220, 70, 70),
        crate::Severity::Unknown => egui::Color32::from_rgb(80, 170, 210),
    }
}

fn mode_label(mode: crate::Mode) -> &'static str {
    match mode {
        crate::Mode::Blind => "盲检",
        crate::Mode::Hash => "哈希",
    }
}

/// GUI 设置：持久化到程序目录 `vine-pulse-settings.json`（绿色版友好）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 强制深度解码（超过大小阈值也解码）
    pub deep: bool,
    /// 全面检测模式（取消全部大小限制，内容级检查全开；慢但最彻底）
    pub full: bool,
    /// 深度解码大小阈值（MB）
    pub max_deep_size_mb: u64,
    /// 低熵判定阈值（0~8）
    pub entropy_threshold: f64,
    /// 外部工具路径覆盖（留空 = 自动探测：程序目录 → tools\ → PATH）
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub sevenzip_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            deep: false,
            full: false,
            max_deep_size_mb: 64,
            entropy_threshold: 0.5,
            ffmpeg_path: String::new(),
            ffprobe_path: String::new(),
            sevenzip_path: String::new(),
        }
    }
}

impl Settings {
    fn settings_file() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("vine-pulse-settings.json")))
            .unwrap_or_else(|| std::env::temp_dir().join("vine-pulse-settings.json"))
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::settings_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<PathBuf, String> {
        let path = Self::settings_file();
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{}：{e}", path.display()))?;
        Ok(path)
    }

    /// 把工具路径覆盖写入进程环境（tools.rs 的查找顺序：环境变量优先）
    fn apply_env(&self) {
        set_env("VINEPULSE_FFMPEG", &self.ffmpeg_path);
        set_env("VINEPULSE_FFPROBE", &self.ffprobe_path);
        set_env("VINEPULSE_7Z", &self.sevenzip_path);
    }

    fn to_opts(&self) -> Opts {
        Opts {
            deep: self.deep,
            full: self.full,
            max_deep_size: self.max_deep_size_mb.saturating_mul(1024 * 1024),
            entropy_threshold: self.entropy_threshold.clamp(0.0, 8.0),
            ..Default::default()
        }
    }
}

fn set_env(key: &str, value: &str) {
    let value = value.trim();
    // SAFETY：仅在 UI 线程（启动/设置保存/开始扫描前）调用，扫描工作线程只读环境变量，
    // 且调用点均在派生扫描线程之前或扫描未运行时，不存在数据竞态。
    unsafe {
        if value.is_empty() {
            std::env::remove_var(key);
        } else {
            std::env::set_var(key, value);
        }
    }
}

fn format_bytes(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    // 必须除以进率再格式化：否则传进来的是字节数，会显示成 "125000.00 MB/s"
    if bps >= GB {
        format!("{:.1} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.0} KB/s", bps / KB)
    } else {
        format!("{:.0} B/s", bps)
    }
}

fn format_eta(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 || secs > 24.0 * 3600.0 {
        return "--:--:--".to_string();
    }
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn export_status(
    kind: &str,
    result: Result<Option<PathBuf>, String>,
) -> String {
    match result {
        Ok(Some(p)) => format!("{kind} 报告已导出：{}", p.display()),
        Ok(None) => "已取消导出".to_string(),
        Err(e) => format!("导出 {kind} 失败：{e}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Scan,
    Settings,
}

pub struct PulseApp {
    settings: Settings,
    settings_draft: Settings,
    page: Page,
    scan: scan_view::ScanState,
    status: String,
}

impl PulseApp {
    fn new() -> Self {
        let settings = Settings::load();
        settings.apply_env();
        Self {
            settings_draft: settings.clone(),
            settings,
            page: Page::Scan,
            scan: scan_view::ScanState::new(),
            status: "就绪：添加文件或文件夹开始扫描（支持拖拽）".to_string(),
        }
    }

    fn start_scan(&mut self) {
        let mut opts = self.settings.to_opts();
        self.settings.apply_env();
        if let Err(e) = self.scan.start(&mut opts) {
            self.status = format!("无法开始扫描：{e}");
        } else {
            self.status = "扫描中…".to_string();
        }
    }
}

impl eframe::App for PulseApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.scan.poll_events(ui.ctx());
        if self.scan.finished_event() {
            let s = Summary::from_reports(self.scan.reports());
            self.status = if self.scan.cancelled() {
                format!(
                    "已取消：完成 {}/{} 个文件（正常 {} · 可疑 {} · 损坏 {}）",
                    s.total,
                    self.scan.total(),
                    s.ok,
                    s.warning,
                    s.corrupt
                )
            } else {
                format!(
                    "扫描完成：共 {} 个文件（正常 {} · 可疑 {} · 损坏 {} · 未知 {}）· 耗时 {:.2}s",
                    s.total,
                    s.ok,
                    s.warning,
                    s.corrupt,
                    s.unknown,
                    self.scan.elapsed().as_secs_f64()
                )
            };
        }

        self.top_bar(ui);
        self.status_bar(ui);
        match self.page {
            Page::Scan => self.scan.ui(ui, &self.status),
            Page::Settings => self.settings_ui(ui),
        }
    }
}

impl PulseApp {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut cancel = false;
        let mut add_files = false;
        let mut add_folder = false;
        let mut clear_targets = false;
        let mut import_manifest = false;
        let mut clear_manifest = false;
        let mut export_json = false;
        let mut export_csv = false;
        let mut export_html = false;
        let mut open_settings = false;
        let mut open_scan = false;

        egui::containers::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let running = self.scan.running();
                if ui.add_enabled(!running, egui::Button::new("➕ 添加文件")).clicked() {
                    add_files = true;
                }
                if ui.add_enabled(!running, egui::Button::new("📁 添加文件夹")).clicked() {
                    add_folder = true;
                }
                if ui.add_enabled(!running && !self.scan.targets().is_empty(), egui::Button::new("🗑 清空目标")).clicked()
                {
                    clear_targets = true;
                }
                ui.separator();
                if ui
                    .add_enabled(!running && !self.scan.targets_empty(), egui::Button::new("▶ 开始扫描"))
                    .on_disabled_hover_text("先添加文件或文件夹")
                    .clicked()
                {
                    start = true;
                }
                if ui
                    .add_enabled(running, egui::Button::new("⏹ 取消"))
                    .clicked()
                {
                    cancel = true;
                }
                ui.separator();
                if ui.add_enabled(!running, egui::Button::new("📋 导入哈希清单")).clicked() {
                    import_manifest = true;
                }
                if ui
                    .add_enabled(!running && self.scan.has_manifest(), egui::Button::new("✕ 清除清单"))
                    .clicked()
                {
                    clear_manifest = true;
                }
                ui.separator();
                let has_reports = !self.scan.reports().is_empty();
                if ui
                    .add_enabled(has_reports, egui::Button::new("导出 JSON"))
                    .clicked()
                {
                    export_json = true;
                }
                if ui
                    .add_enabled(has_reports, egui::Button::new("导出 CSV"))
                    .clicked()
                {
                    export_csv = true;
                }
                if ui
                    .add_enabled(has_reports, egui::Button::new("导出 HTML"))
                    .clicked()
                {
                    export_html = true;
                }
                ui.separator();
                match self.page {
                    Page::Scan if ui.button("⚙ 设置").clicked() => open_settings = true,
                    Page::Settings if ui.button("← 返回扫描").clicked() => open_scan = true,
                    _ => {}
                }
            });
        });

        if add_files
            && let Some(paths) = rfd::FileDialog::new()
                .add_filter("所有文件", &["*"])
                .pick_files()
        {
            self.scan.add_targets(paths);
            self.status = format!("已添加文件，当前目标 {} 个", self.scan.targets().len());
        }
        if add_folder
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            self.scan.add_targets(vec![dir]);
            self.status = format!("已添加文件夹，当前目标 {} 个", self.scan.targets().len());
        }
        if clear_targets {
            self.scan.clear_targets();
            self.status = "已清空扫描目标".to_string();
        }
        if import_manifest {
            match self.scan.import_manifest() {
                Ok(true) => {
                    let (name, n) = self.scan.manifest_info();
                    self.status = format!("已导入清单 {name}（{n} 个文件，将以清单为扫描目标并比对哈希）");
                }
                Ok(false) => {}
                Err(e) => self.status = format!("导入清单失败：{e}"),
            }
        }
        if clear_manifest {
            self.scan.clear_manifest();
            self.status = "已清除哈希清单".to_string();
        }
        if start {
            self.start_scan();
        }
        if cancel {
            self.scan.request_cancel();
            self.status = "正在取消…".to_string();
        }
        // 仅在导出时克隆全量报告——之前每帧 to_vec() 会在大结果集上白白复制
        // 整个 Vec<Report>，是大批量扫描时 UI 掉帧的主因之一。
        if export_json || export_csv || export_html {
            let reports = self.scan.reports().to_vec();
            if export_json {
                self.status = export_status("JSON", detail_view::export_json(&reports));
            }
            if export_csv {
                self.status = export_status("CSV", detail_view::export_csv(&reports));
            }
            if export_html {
                self.status = export_status("HTML", detail_view::export_html(&reports));
            }
        }
        if open_settings {
            self.settings_draft = self.settings.clone();
            self.page = Page::Settings;
        }
        if open_scan {
            self.page = Page::Scan;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::containers::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.scan.running() {
                    ui.spinner();
                    let (done, total) = self.scan.progress();
                    ui.label(format!("扫描中 {done}/{total}"));
                    let failed = self.scan.failed_count();
                    ui.separator();
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("失败 {failed}"),
                    );
                    let elapsed = self.scan.elapsed();
                    if elapsed.as_secs_f64() > 0.5 {
                        let speed = self.scan.smoothed_speed();
                        ui.separator();
                        ui.label(format_bytes(speed));
                        if total > 0 && done > 0 && done < total {
                            let eta_secs =
                                elapsed.as_secs_f64() * (total - done) as f64 / done as f64;
                            ui.separator();
                            ui.label(format!("ETA {}", format_eta(eta_secs)));
                        }
                    }
                    let frac = if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    };
                    ui.add(egui::ProgressBar::new(frac).desired_width(140.0));
                    if ui.button("取消").clicked() {
                        self.scan.request_cancel();
                    }
                } else {
                    ui.label(&self.status);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ff = if tools::ffmpeg_available() {
                        "可用"
                    } else {
                        "不可用"
                    };
                    let sz = if tools::seven_zip().is_some() {
                        "可用"
                    } else {
                        "不可用"
                    };
                    ui.label(format!("深度解码工具：ffmpeg {ff} · 7z {sz}"));
                });
            });
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut save = false;
        let mut reset = false;
        egui::containers::CentralPanel::default().show(ui, |ui| {
            ui.add_space(6.0);
            ui.heading("设置");
            ui.add_space(6.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.checkbox(
                    &mut self.settings_draft.full,
                    "全面检测模式（取消大小限制，深度解码与内容级检查全开；慢但最彻底）",
                );
                ui.checkbox(
                    &mut self.settings_draft.deep,
                    "强制深度解码（超过大小阈值的文件也做完整解码）",
                );
                ui.add(
                    egui::DragValue::new(&mut self.settings_draft.max_deep_size_mb)
                        .range(1..=4096)
                        .suffix(" MB")
                        .speed(1.0),
                );
                ui.label("深度解码大小阈值：小于等于该大小的文件总是做深度解码");
                ui.add(
                    egui::DragValue::new(&mut self.settings_draft.entropy_threshold)
                        .range(0.0..=8.0)
                        .speed(0.05),
                );
                ui.label("低熵判定阈值（0~8，默认 0.5）");

                ui.add_space(10.0);
                ui.heading("外部工具路径（留空自动探测）");
                ui.label("查找顺序：程序同目录 → tools\\ 子目录 → PATH；填写后立即生效");
                tool_path_row(ui, "ffmpeg 路径：", &mut self.settings_draft.ffmpeg_path);
                tool_path_row(ui, "ffprobe 路径：", &mut self.settings_draft.ffprobe_path);
                tool_path_row(ui, "7z 路径：", &mut self.settings_draft.sevenzip_path);

                ui.add_space(10.0);
                let ff = if tools::ffmpeg_available() {
                    "ffmpeg/ffprobe 可用（音视频可深度解码）"
                } else {
                    "ffmpeg/ffprobe 不可用（音视频仅做结构校验，界面已降级）"
                };
                let sz = if tools::seven_zip().is_some() {
                    "7z 可用"
                } else {
                    "7z 不可用（7z/RAR 仅做结构校验）"
                };
                ui.label(ff);
                ui.label(sz);

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button("保存设置").clicked() {
                        save = true;
                    }
                    if ui.button("恢复默认").clicked() {
                        reset = true;
                    }
                });
                ui.add_space(6.0);
                ui.label(format!("设置文件：{}", Settings::settings_file().display()));
            });
        });
        if save {
            self.settings = self.settings_draft.clone();
            self.settings.apply_env();
            match self.settings.save() {
                Ok(p) => self.status = format!("设置已保存：{}", p.display()),
                Err(e) => self.status = format!("保存设置失败：{e}"),
            }
        }
        if reset {
            self.settings_draft = Settings::default();
        }
    }
}

fn tool_path_row(ui: &mut egui::Ui, label: &str, path: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::TextEdit::singleline(path)
                .desired_width(360.0)
                .hint_text("留空 = 自动探测"),
        );
        if ui.button("浏览…").clicked()
            && let Some(p) = rfd::FileDialog::new()
                .add_filter("可执行文件", &["exe"])
                .pick_file()
        {
            *path = p.display().to_string();
        }
    });
}
