use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::core::paths::{is_reparse_point, to_long_path};
use crate::ui::i18n;

pub struct EntryRow {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
    pub reparse: bool,
}

pub fn list_dir(path: &Path) -> std::io::Result<Vec<EntryRow>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(to_long_path(path))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = meta.is_dir();
        rows.push(EntryRow {
            name,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            reparse: is_reparse_point(&meta),
        });
    }
    rows.sort_by(|a, b| (a.is_dir, &a.name).cmp(&(b.is_dir, &b.name)));
    Ok(rows)
}

pub enum PaneEvent {
    Activate(usize),
    Navigate(usize, PathBuf),
    OpenFile(PathBuf),
    EditAddress(usize, String),
    AddressFocus(usize),
    AddressDone(usize),
    Refresh(usize),
    GoBack(usize),
    GoForward(usize),
    GoUp(usize),
    PickFolder(usize),
    ContextMenu(usize, egui::Pos2, MenuTarget),
    DragDrop(usize, Vec<PathBuf>, PathBuf),
    Rename(usize, String, String),
}

#[derive(Clone)]
pub struct ItemDrag {
    pub paths: Vec<PathBuf>,
}

#[derive(Clone)]
pub enum MenuTarget {
    Items(Vec<PathBuf>),
    Background,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    List,
    Grid,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::List => i18n::t("列表", "Details"),
            ViewMode::Grid => i18n::t("图标", "Icons"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Date,
    Type,
    Size,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => i18n::t("名称", "Name"),
            SortKey::Date => i18n::t("修改日期", "Date modified"),
            SortKey::Type => i18n::t("类型", "Type"),
            SortKey::Size => i18n::t("大小", "Size"),
        }
    }

    pub const ALL: [SortKey; 4] = [SortKey::Name, SortKey::Date, SortKey::Type, SortKey::Size];
}

pub struct Pane {
    pub history: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub current: PathBuf,
    pub entries: Vec<EntryRow>,
    pub error: Option<String>,
    pub address: String,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub sort: SortKey,
    pub sort_asc: bool,
    pub filter: String,
    pub view_mode: ViewMode,
    pub marquee: Option<(egui::Pos2, egui::Pos2)>,
    pub renaming: Option<usize>,
    pub rename_text: String,
    pub rename_focused: bool,
    pub last_click: Option<(usize, f64)>,
    /// 慢二次点击的改名意向：延迟到可能的双击窗口结束再生效。
    pub pending_rename: Option<(usize, f64)>,
}

pub struct PaneGroup {
    pub tabs: Vec<Pane>,
    pub active: usize,
}

impl PaneGroup {
    pub fn new(current: PathBuf) -> Self {
        PaneGroup {
            tabs: vec![Pane::new(current)],
            active: 0,
        }
    }

    pub fn active(&self) -> &Pane {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    pub fn active_mut(&mut self) -> &mut Pane {
        let i = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[i]
    }

    pub fn add_tab(&mut self, current: PathBuf) {
        self.tabs.push(Pane::new(current));
        self.active = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 || idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
    }

    pub fn reload_all(&mut self) {
        for t in &mut self.tabs {
            t.reload();
        }
    }

    pub fn tab_label(&self, idx: usize) -> String {
        let p = &self.tabs[idx];
        p.current
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| p.current.to_string_lossy().to_string())
    }
}

impl Pane {
    pub fn new(current: PathBuf) -> Self {
        let mut pane = Pane {
            history: Vec::new(),
            forward: Vec::new(),
            current,
            entries: Vec::new(),
            error: None,
            address: String::new(),
            selected: BTreeSet::new(),
            anchor: None,
            sort: SortKey::Name,
            sort_asc: true,
            filter: String::new(),
            view_mode: ViewMode::List,
            marquee: None,
            renaming: None,
            rename_text: String::new(),
            rename_focused: false,
            last_click: None,
            pending_rename: None,
        };
        pane.reload();
        pane
    }

    pub fn reload(&mut self) {
        let keep: BTreeSet<String> = self
            .selected
            .iter()
            .filter_map(|i| self.entries.get(*i))
            .map(|e| e.name.clone())
            .collect();
        match list_dir(&self.current) {
            Ok(rows) => {
                self.entries = rows;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(format!("{}：{e}", i18n::t("读取目录失败", "Failed to read folder")));
            }
        }
        self.sort_entries();
        self.address = self.current.to_string_lossy().to_string();
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| keep.contains(&e.name))
            .map(|(i, _)| i)
            .collect();
        self.anchor = None;
        self.renaming = None;
        self.rename_focused = false;
        self.pending_rename = None;
        self.last_click = None;
    }

    pub fn sort_entries(&mut self) {
        let key = self.sort;
        let asc = self.sort_asc;
        self.entries.sort_by(|a, b| {
            let grp = b.is_dir.cmp(&a.is_dir);
            if grp != std::cmp::Ordering::Equal {
                return grp;
            }
            let ord = match key {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Date => a.modified.cmp(&b.modified),
                SortKey::Type => {
                    type_label(&a.name, a.is_dir).cmp(&type_label(&b.name, b.is_dir))
                }
                SortKey::Size => a.size.cmp(&b.size),
            };
            let ord = ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            if asc { ord } else { ord.reverse() }
        });
    }

    pub fn view_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let f = self.filter.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&f))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn select_all(&mut self) {
        let view = self.view_indices();
        self.anchor = view.first().copied();
        self.selected = view.into_iter().collect();
    }

    pub fn can_back(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn can_up(&self) -> bool {
        self.current.parent().is_some()
    }

    pub fn navigate(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.history.push(std::mem::take(&mut self.current));
            self.forward.clear();
            self.current = path;
            self.selected.clear();
            self.anchor = None;
            self.reload();
        }
    }

    pub fn go_back(&mut self) {
        if let Some(prev) = self.history.pop() {
            self.forward.push(std::mem::take(&mut self.current));
            self.current = prev;
            self.selected.clear();
            self.reload();
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(next) = self.forward.pop() {
            self.history.push(std::mem::take(&mut self.current));
            self.current = next;
            self.selected.clear();
            self.reload();
        }
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = self.current.parent().map(|p| p.to_path_buf()) {
            self.navigate(parent);
        }
    }

    pub fn select(&mut self, idx: usize, ctrl: bool, shift: bool) {
        if shift {
            let anchor = self.anchor.unwrap_or(idx);
            let (lo, hi) = if anchor <= idx { (anchor, idx) } else { (idx, anchor) };
            if !ctrl {
                self.selected.clear();
            }
            for i in lo..=hi {
                self.selected.insert(i);
            }
        } else if ctrl {
            if !self.selected.remove(&idx) {
                self.selected.insert(idx);
            }
            self.anchor = Some(idx);
        } else {
            self.selected.clear();
            self.selected.insert(idx);
            self.anchor = Some(idx);
        }
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected
            .iter()
            .filter_map(|i| self.entries.get(*i))
            .map(|e| self.current.join(&e.name))
            .collect()
    }
}

fn norm_path(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// 内部拖放的取消语义：源等于目标、或源原本就在目标目录里（拖回原父目录）时剔除，
/// 过滤后为空则整体不动作（Explorer 取消语义）。
pub fn filter_drop_paths(paths: &[PathBuf], dest: &Path) -> Vec<PathBuf> {
    let d = norm_path(dest);
    paths
        .iter()
        .filter(|p| {
            norm_path(p) != d && p.parent().map(norm_path).as_deref() != Some(d.as_str())
        })
        .cloned()
        .collect()
}

pub fn format_size(n: u64) -> String {
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

pub fn format_time(t: Option<std::time::SystemTime>) -> String {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| super::format_ts(d.as_secs() as i64))
        .unwrap_or_else(|| "-".into())
}

fn label_width(ui: &egui::Ui, text: &str) -> f32 {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(text.to_string(), font_id.clone(), egui::Color32::WHITE)
            .size()
            .x
    })
}

pub fn type_label(name: &str, is_dir: bool) -> String {
    if is_dir {
        return i18n::t("文件夹", "Folder").to_string();
    }
    let ext = std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let kind: &str = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff" => {
            i18n::t("图片", "image")
        }
        "mp3" | "wav" | "flac" | "ape" | "m4a" | "ogg" | "aac" => i18n::t("音频", "audio"),
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => i18n::t("视频", "video"),
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" => i18n::t("压缩文件", "archive"),
        "exe" | "msi" => i18n::t("应用程序", "application"),
        "txt" | "log" | "md" | "ini" | "cfg" | "toml" | "yaml" | "yml" => {
            i18n::t("文本文档", "text document")
        }
        "pdf" => i18n::t("PDF 文档", "PDF document"),
        "doc" | "docx" => i18n::t("Word 文档", "Word document"),
        "xls" | "xlsx" => i18n::t("Excel 工作表", "Excel spreadsheet"),
        "ppt" | "pptx" => i18n::t("PowerPoint 演示文稿", "PowerPoint presentation"),
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "hpp" | "java" | "go"
        | "cs" | "sh" | "ps1" => i18n::t("源代码文件", "source file"),
        "" => i18n::t("文件", "file"),
        _ => return format!("{} {}", ext.to_uppercase(), i18n::t("文件", "file")),
    };
    kind.to_string()
}

fn render_breadcrumb(
    ui: &mut egui::Ui,
    pane: &Pane,
    pane_idx: usize,
    max_w: f32,
    events: &mut Vec<PaneEvent>,
) {
    let mut segs: Vec<(String, PathBuf)> = Vec::new();
    let mut p = Some(pane.current.as_path());
    while let Some(cur) = p {
        let label = cur
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| cur.to_string_lossy().to_string());
        segs.push((label, cur.to_path_buf()));
        p = cur.parent();
    }
    segs.reverse();

    let widths: Vec<f32> = segs
        .iter()
        .map(|(l, _)| label_width(ui, l) + 18.0)
        .collect();
    let sep_w = 16.0f32;
    let mut total: f32 =
        widths.iter().sum::<f32>() + sep_w * (segs.len().saturating_sub(1)) as f32;
    let mut start = 0;
    let mut dots = false;
    while segs.len() - start > 1 && total > max_w {
        total -= widths[start] + sep_w;
        start += 1;
        dots = true;
    }

    let before = ui.cursor().min.x;
    let sep = |ui: &mut egui::Ui| {
        ui.add(egui::Label::new(
            egui::RichText::new("›").weak().size(15.0),
        ));
    };
    if dots {
        ui.label(egui::RichText::new("…").weak());
        sep(ui);
    }
    for (i, (label, path)) in segs[start..].iter().enumerate() {
        if i > 0 || dots {
            sep(ui);
        }
        let resp = ui.add(egui::Button::new(label.as_str()));
        if resp.clicked() {
            events.push(PaneEvent::Navigate(pane_idx, path.clone()));
        }
    }
    let used = ui.cursor().min.x - before;
    let rem = (max_w - used).max(0.0);
    if rem > 8.0 {
        let (_, resp) = ui.allocate_exact_size(egui::vec2(rem, 18.0), egui::Sense::click());
        if resp.clicked() {
            events.push(PaneEvent::AddressFocus(pane_idx));
        }
    }
}

fn header_cell(
    ui: &mut egui::Ui,
    text: &str,
    right: bool,
    arrow: Option<bool>,
    pane_idx: usize,
) -> bool {
    let rect = ui.max_rect();
    let resp = ui.interact(
        rect,
        ui.id().with(format!("hdr_{pane_idx}_{text}")),
        egui::Sense::click(),
    );
    let bg = if resp.hovered() {
        super::theme::HOVER
    } else {
        super::theme::HEADER_BG
    };
    ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, bg);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, super::theme::BORDER),
    );
    ui.painter().vline(
        rect.right(),
        rect.y_range(),
        egui::Stroke::new(1.0, super::theme::BORDER),
    );
    let full = match arrow {
        Some(true) => format!("{text} ▲"),
        Some(false) => format!("{text} ▼"),
        None => text.to_string(),
    };
    if right {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(full).strong().size(12.0));
        });
    } else {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(full).strong().size(12.0));
    }
    resp.clicked()
}

pub enum TabEvent {
    Add(usize),
    Close(usize, usize),
    Switch(usize, usize),
}

#[allow(clippy::too_many_arguments)]
pub fn render_group(
    ui: &mut egui::Ui,
    pane_idx: usize,
    group: &mut PaneGroup,
    focus_address: bool,
    thumbs: &mut super::thumbs::ThumbCache,
    folder_peek: &mut super::thumbs::FolderPeek,
    icons: &mut super::shell_icons::ShellIcons,
    is_active: bool,
) -> (Vec<PaneEvent>, Vec<TabEvent>) {
    let mut tab_events = Vec::new();
    let mut pre_events: Vec<PaneEvent> = Vec::new();
    let group_rect = ui.available_rect_before_wrap();
    if is_active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                group_rect.min,
                egui::vec2(group_rect.width(), 3.0),
            ),
            0.0,
            super::theme::ACCENT,
        );
    }
    super::theme::tab_strip_frame().show(ui, |ui| {
        super::theme::flat_buttons(ui);
        ui.horizontal(|ui| {
            for i in 0..group.tabs.len() {
                let active = group.active == i;
                let label = group.tab_label(i);
                let text = if active {
                    egui::RichText::new(&label).strong()
                } else {
                    egui::RichText::new(&label).weak()
                };
                let resp = ui
                    .scope(|ui| {
                        if active {
                            let v = ui.visuals_mut();
                            v.selection.bg_fill = egui::Color32::WHITE;
                            v.selection.stroke = egui::Stroke::new(1.0, super::theme::TEXT);
                        }
                        ui.add(
                            egui::Button::selectable(active, text)
                                .corner_radius(super::theme::TAB_TOP),
                        )
                    })
                    .inner;
                if resp.clicked() {
                    tab_events.push(TabEvent::Switch(pane_idx, i));
                    pre_events.push(PaneEvent::Activate(pane_idx));
                }
                if active && group.tabs.len() > 1 && ui.small_button("×").clicked() {
                    tab_events.push(TabEvent::Close(pane_idx, i));
                }
            }
            if ui
                .small_button("+")
                .on_hover_text(i18n::t("新建标签", "New Tab"))
                .clicked()
            {
                tab_events.push(TabEvent::Add(pane_idx));
                pre_events.push(PaneEvent::Activate(pane_idx));
            }
        });
    });
    let mut events = pre_events;
    events.extend(render_pane(
        ui,
        pane_idx,
        group.active_mut(),
        focus_address,
        thumbs,
        folder_peek,
        icons,
    ));
    (events, tab_events)
}

pub fn render_pane(
    ui: &mut egui::Ui,
    pane_idx: usize,
    pane: &mut Pane,
    focus_address: bool,
    thumbs: &mut super::thumbs::ThumbCache,
    folder_peek: &mut super::thumbs::FolderPeek,
    icons: &mut super::shell_icons::ShellIcons,
) -> Vec<PaneEvent> {
    let mut events: Vec<PaneEvent> = Vec::new();

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let can_back = pane.can_back();
        let b = ui.add_enabled(can_back, egui::Button::new("←"));
        let b = if can_back {
            b.on_hover_text(i18n::t("后退 (Alt+←)", "Back (Alt+←)"))
        } else {
            b.on_disabled_hover_text(i18n::t("没有更早的记录", "No previous pages"))
        };
        if b.clicked() {
            events.push(PaneEvent::GoBack(pane_idx));
        }
        let can_fwd = pane.can_forward();
        let b = ui.add_enabled(can_fwd, egui::Button::new("→"));
        let b = if can_fwd {
            b.on_hover_text(i18n::t("前进 (Alt+→)", "Forward (Alt+→)"))
        } else {
            b.on_disabled_hover_text(i18n::t("没有更新的记录", "No later pages"))
        };
        if b.clicked() {
            events.push(PaneEvent::GoForward(pane_idx));
        }
        let can_up = pane.can_up();
        let b = ui.add_enabled(can_up, egui::Button::new("↑"));
        let b = if can_up {
            b.on_hover_text(i18n::t("上一级 (Alt+↑)", "Up (Alt+↑)"))
        } else {
            b.on_disabled_hover_text(i18n::t("已在顶级目录", "Already at the top"))
        };
        if b.clicked() {
            events.push(PaneEvent::GoUp(pane_idx));
        }

        let search_w = if ui.available_width() < 520.0 { 110.0 } else { 160.0 };
        let right_w = search_w + 34.0 + ui.spacing().item_spacing.x * 2.0;
        let addr_w = (ui.available_width() - right_w).max(60.0);

        if focus_address {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut pane.address)
                    .id_salt(format!("addr_{pane_idx}"))
                    .desired_width(addr_w)
                    .cursor_at_end(true)
                    .hint_text(i18n::t(
                        "输入路径后回车 (Ctrl+L)",
                        "Type a path and press Enter (Ctrl+L)",
                    )),
            );
            let enter = ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));
            if resp.has_focus() && enter {
                let addr = pane.address.trim().trim_matches('"').to_string();
                events.push(PaneEvent::EditAddress(pane_idx, addr));
                events.push(PaneEvent::AddressDone(pane_idx));
            } else if resp.lost_focus() {
                if enter {
                    let addr = pane.address.trim().trim_matches('"').to_string();
                    events.push(PaneEvent::EditAddress(pane_idx, addr));
                }
                events.push(PaneEvent::AddressDone(pane_idx));
            } else if !resp.has_focus() {
                resp.request_focus();
            }
        } else {
            render_breadcrumb(ui, pane, pane_idx, addr_w, &mut events);
        }

        let mut refresh = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut pane.filter)
                    .id_salt(format!("filter_{pane_idx}"))
                    .desired_width(search_w)
                    .hint_text(i18n::t("搜索", "Search")),
            )
            .on_hover_text(i18n::t(
                "在当前文件夹内即时搜索",
                "Filter entries in this folder",
            ));
            if ui
                .button("⟳")
                .on_hover_text(format!("{} (F5)", i18n::t("刷新", "Refresh")))
                .clicked()
            {
                refresh = true;
            }
        });
        if refresh {
            events.push(PaneEvent::Refresh(pane_idx));
        }
    });

    let mut row_action: Option<RowAction> = None;
    let mut item_menu_fired = false;
    let view = pane.view_indices();
    if !pane.filter.is_empty() {
        let vs: BTreeSet<usize> = view.iter().copied().collect();
        pane.selected.retain(|i| vs.contains(i));
        if pane.anchor.is_some_and(|a| !vs.contains(&a)) {
            pane.anchor = None;
        }
    }
    let entries_len = view.len();
    let (cur_key, cur_asc) = (pane.sort, pane.sort_asc);
    let arrow = |k: SortKey| if cur_key == k { Some(cur_asc) } else { None };
    let mut new_sort: Option<SortKey> = None;
    let mut cell_rects: Vec<(usize, egui::Rect)> = Vec::new();

    let now = ui.input(|i| i.time);
    // 慢二次点击的改名意向延迟生效：等待 egui 双击窗口（0.3s）过去，
    // 若这其实是双击的首击，double_clicked 分支会先把它清掉。
    if let Some((idx, t)) = pane.pending_rename
        && now - t >= 0.32
    {
        pane.pending_rename = None;
        if pane.view_mode == ViewMode::List
            && pane.renaming.is_none()
            && pane.selected.contains(&idx)
            && pane.selected.len() == 1
            && pane.entries.get(idx).is_some()
        {
            pane.renaming = Some(idx);
            pane.rename_text = pane.entries[idx].name.clone();
            pane.rename_focused = false;
        }
    }

    let content_rect = ui.available_rect_before_wrap();
    let cover = ui.interact(
        content_rect,
        ui.id().with(format!("pane_cover_{pane_idx}")),
        egui::Sense::click_and_drag(),
    );

    if pane.view_mode == ViewMode::Grid {
        render_grid(
            ui,
            pane_idx,
            pane,
            &view,
            &mut events,
            &mut row_action,
            &mut item_menu_fired,
            thumbs,
            folder_peek,
            icons,
            &mut cell_rects,
        );
    } else {
        let avail_w = ui.available_width();
        let name_w = (avail_w * 0.40).max(80.0);
        let date_w = (avail_w * 0.22).max(60.0);
        let type_w = (avail_w * 0.18).max(50.0);
        let size_w = (avail_w * 0.15).max(40.0);
        let mods = ui.input(|i| (i.modifiers.ctrl || i.modifiers.command, i.modifiers.shift));
        egui_extras::TableBuilder::new(ui)
        .id_salt(format!("pane_table_{pane_idx}"))
        .striped(true)
        .resizable(false)
        .sense(egui::Sense::click_and_drag())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(egui_extras::Column::initial(name_w).at_least(60.0).resizable(false))
        .column(egui_extras::Column::initial(date_w).at_least(40.0).resizable(false))
        .column(egui_extras::Column::initial(type_w).at_least(30.0).resizable(false))
        .column(egui_extras::Column::initial(size_w).at_least(30.0).resizable(false))
        .header(26.0, |mut header| {
            header.col(|ui| {
                if header_cell(ui, i18n::t("名称", "Name"), false, arrow(SortKey::Name), pane_idx) {
                    new_sort = Some(SortKey::Name);
                }
            });
            header.col(|ui| {
                if header_cell(
                    ui,
                    i18n::t("修改日期", "Date modified"),
                    false,
                    arrow(SortKey::Date),
                    pane_idx,
                ) {
                    new_sort = Some(SortKey::Date);
                }
            });
            header.col(|ui| {
                if header_cell(ui, i18n::t("类型", "Type"), false, arrow(SortKey::Type), pane_idx) {
                    new_sort = Some(SortKey::Type);
                }
            });
            header.col(|ui| {
                if header_cell(ui, i18n::t("大小", "Size"), true, arrow(SortKey::Size), pane_idx) {
                    new_sort = Some(SortKey::Size);
                }
            });
        })
        .body(|body| {
            body.rows(24.0, entries_len, |mut row| {
                let idx = view[row.index()];
                let (name, is_dir, size, reparse, modified) = {
                    let e = &pane.entries[idx];
                    (
                        e.name.clone(),
                        e.is_dir,
                        e.size,
                        e.reparse,
                        e.modified,
                    )
                };
                let is_sel = pane.selected.contains(&idx);
                row.set_selected(is_sel);
                let mut menu_pos: Option<egui::Pos2> = None;

                let is_renaming = pane.renaming == Some(idx);
                row.col(|ui| {
                    let path = pane.current.join(&name);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if let Some(tex) =
                            icons.icon_for(ui.ctx(), &path, is_dir, reparse, 16)
                        {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            ui.painter().image(
                                tex.id(),
                                rect,
                                egui::Rect::from_min_max(
                                    egui::Pos2::ZERO,
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                        if is_renaming {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut pane.rename_text)
                                    .id_salt(format!("rename_{pane_idx}_{idx}"))
                                    .desired_width(name_w - 24.0)
                                    .clip_text(false),
                            );
                            if !pane.rename_focused {
                                resp.request_focus();
                                pane.rename_focused = true;
                                if let Some(mut state) =
                                    egui::TextEdit::load_state(ui.ctx(), resp.id)
                                {
                                    let stem_end = if is_dir {
                                        name.chars().count()
                                    } else {
                                        match name.rfind('.') {
                                            Some(0) | None => name.chars().count(),
                                            Some(b) => name[..b].chars().count(),
                                        }
                                    };
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(stem_end),
                                        ),
                                    ));
                                    state.store(ui.ctx(), resp.id);
                                }
                            }
                            let enter =
                                ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let escape =
                                ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if escape {
                                pane.renaming = None;
                                pane.rename_focused = false;
                            } else if enter || resp.lost_focus() {
                                let new_name =
                                    pane.rename_text.trim().trim_matches('"').to_string();
                                pane.renaming = None;
                                pane.rename_focused = false;
                                if !new_name.is_empty() && new_name != name {
                                    events.push(PaneEvent::Rename(
                                        pane_idx,
                                        name.clone(),
                                        new_name,
                                    ));
                                }
                            }
                        } else {
                            ui.add(egui::Label::new(&name).truncate());
                        }
                    });
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(format_time(modified)).truncate());
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(type_label(&name, is_dir)).truncate());
                });
                row.col(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add_space(8.0);
                            ui.label(if is_dir { "-".into() } else { format_size(size) });
                        },
                    );
                });

                let row_resp = row.response().clone();
                row_resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                cell_rects.push((idx, row_resp.rect));
                if row_resp.drag_started_by(egui::PointerButton::Primary) {
                    if !is_sel {
                        pane.select(idx, false, false);
                    }
                    let paths = pane.selected_paths();
                    row_resp.clone().dnd_set_drag_payload(ItemDrag { paths });
                }
                if row_resp.clicked() && !is_renaming {
                    let on_name = row_resp
                        .interact_pointer_pos()
                        .map(|p| p.x < row_resp.rect.min.x + name_w)
                        .unwrap_or(false);
                    let slow_second = pane
                        .last_click
                        .is_some_and(|(li, lt)| li == idx && now - lt > 0.5);
                    pane.last_click = Some((idx, now));
                    if slow_second && is_sel && pane.selected.len() == 1 && on_name {
                        // 只登记意向：若这其实是双击的首击，同一/后续帧的
                        // double_clicked 会清掉它，不会闪出改名框。
                        pane.pending_rename = Some((idx, now));
                    } else {
                        pane.pending_rename = None;
                        pane.select(idx, mods.0, mods.1);
                    }
                }
                if row_resp.double_clicked() {
                    pane.last_click = None;
                    pane.pending_rename = None;
                    pane.renaming = None;
                    pane.rename_focused = false;
                    let p = pane.current.join(&name);
                    if is_dir {
                        row_action = Some(RowAction::Navigate(p));
                    } else {
                        row_action = Some(RowAction::Open(p));
                    }
                }
                if row_resp.secondary_clicked() {
                    if !is_sel {
                        pane.select(idx, false, false);
                    }
                    menu_pos = row_resp.interact_pointer_pos();
                }
                if menu_pos.is_none() && row_resp.secondary_clicked() {
                    menu_pos = row_resp.interact_pointer_pos();
                }
                if let Some(pos) = menu_pos {
                    item_menu_fired = true;
                    events.push(PaneEvent::Activate(pane_idx));
                    events.push(PaneEvent::ContextMenu(
                        pane_idx,
                        pos,
                        MenuTarget::Items(pane.selected_paths()),
                    ));
                }
            });
        });
    }

    if let Some(k) = new_sort {
        if pane.sort == k {
            pane.sort_asc = !pane.sort_asc;
        } else {
            pane.sort = k;
            pane.sort_asc = true;
        }
        pane.sort_entries();
        pane.selected.clear();
        pane.anchor = None;
    }
    if !pane.filter.is_empty() && view.is_empty() {
        ui.label(
            egui::RichText::new(i18n::t("无匹配项", "No matching items")).weak(),
        );
    }

    if let Some(action) = row_action {
        match action {
            RowAction::Navigate(p) => events.push(PaneEvent::Navigate(pane_idx, p)),
            RowAction::Open(p) => events.push(PaneEvent::OpenFile(p)),
        }
    }

    if let Some(err) = &pane.error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }

    // 行/格子的整体包围盒（含 3px 行间隙）：起始于此的按压一律视为“点在列表上”，
    // 绝不触发 cover 的清选择/框选。
    let list_span = cell_rects
        .iter()
        .map(|(_, r)| *r)
        .reduce(|a, b| a.union(b));
    let press_on_list = |resp: &egui::Response| -> bool {
        resp.interact_pointer_pos()
            .is_some_and(|p| list_span.is_some_and(|s| s.contains(p)))
    };

    if cover.contains_pointer() {
        if cover.dnd_hover_payload::<ItemDrag>().is_some() {
            ui.painter().rect_filled(
                content_rect,
                0.0,
                egui::Color32::from_white_alpha(20),
            );
            ui.painter().rect_stroke(
                content_rect,
                0.0,
                egui::Stroke::new(2.0, super::theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(payload) = cover.dnd_release_payload::<ItemDrag>() {
            let dest = pane.current.clone();
            let paths = filter_drop_paths(&payload.paths, &dest);
            if !paths.is_empty() {
                events.push(PaneEvent::DragDrop(pane_idx, paths, dest));
            }
        }
        if cover.drag_started_by(egui::PointerButton::Primary) {
            let origin = cover.interact_pointer_pos().unwrap_or(content_rect.min);
            let on_row = list_span.is_some_and(|s| s.contains(origin));
            if !on_row {
                events.push(PaneEvent::Activate(pane_idx));
                pane.marquee = Some((origin, origin));
                if !ui.input(|i| i.modifiers.ctrl || i.modifiers.command) {
                    pane.selected.clear();
                }
            }
        }
        if cover.dragged_by(egui::PointerButton::Primary) {
            let cur = cover.interact_pointer_pos().unwrap_or(content_rect.min);
            if let Some((start, _)) = pane.marquee {
                pane.marquee = Some((start, cur));
            }
        }
        if cover.drag_stopped_by(egui::PointerButton::Primary)
            && let Some((a, b)) = pane.marquee.take()
        {
            let r = egui::Rect::from_two_pos(a, b);
            if r.width() > 3.0 || r.height() > 3.0 {
                apply_marquee(pane, &cell_rects, r);
            }
        }
        if cover.clicked_by(egui::PointerButton::Primary)
            && pane.marquee.is_none()
            && !press_on_list(&cover)
        {
            if !ui.input(|i| i.modifiers.ctrl || i.modifiers.command) {
                pane.selected.clear();
                pane.anchor = None;
            }
            events.push(PaneEvent::Activate(pane_idx));
        }
        if !item_menu_fired
            && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary))
        {
            let pos = ui
                .input(|i| i.pointer.interact_pos())
                .unwrap_or(content_rect.min);
            events.push(PaneEvent::Activate(pane_idx));
            events.push(PaneEvent::ContextMenu(
                pane_idx,
                pos,
                MenuTarget::Background,
            ));
        }
    }
    if let Some((a, b)) = pane.marquee {
        let r = egui::Rect::from_two_pos(a, b);
        ui.painter().rect_filled(
            r,
            0.0,
            egui::Color32::from_white_alpha(30),
        );
        ui.painter().rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(0x00, 0x78, 0xD4)),
            egui::StrokeKind::Inside,
        );
    }

    events
}

#[allow(clippy::too_many_arguments)]
fn render_grid(
    ui: &mut egui::Ui,
    pane_idx: usize,
    pane: &mut Pane,
    view: &[usize],
    events: &mut Vec<PaneEvent>,
    row_action: &mut Option<RowAction>,
    item_menu_fired: &mut bool,
    thumbs: &mut super::thumbs::ThumbCache,
    folder_peek: &mut super::thumbs::FolderPeek,
    icons: &mut super::shell_icons::ShellIcons,
    cell_rects: &mut Vec<(usize, egui::Rect)>,
) {
    const CELL_W: f32 = 96.0;
    const CELL_H: f32 = 100.0;
    const ICON: f32 = 56.0;
    let cols = ((ui.available_width() / (CELL_W + 6.0)).floor() as usize).max(1);
    let nrows = view.len().div_ceil(cols);
    egui::ScrollArea::vertical()
        .id_salt(format!("pane_grid_{pane_idx}"))
        .auto_shrink([false, false])
        .show_rows(ui, CELL_H + 6.0, nrows, |ui, row_range| {
            for r in row_range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for c in 0..cols {
                        let vi = r * cols + c;
                        let cell = egui::vec2(CELL_W, CELL_H);
                        if vi >= view.len() {
                            ui.allocate_exact_size(cell, egui::Sense::hover());
                            continue;
                        }
                        let idx = view[vi];
                        let (name, is_dir, reparse) = {
                            let e = &pane.entries[idx];
                            (e.name.clone(), e.is_dir, e.reparse)
                        };
                        let path = pane.current.join(&name);
                        let is_sel = pane.selected.contains(&idx);
                        let (rect, resp) =
                            ui.allocate_exact_size(cell, egui::Sense::click_and_drag());
                        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            let paths = if is_sel {
                                pane.selected_paths()
                            } else {
                                vec![path.clone()]
                            };
                            resp.clone().dnd_set_drag_payload(ItemDrag { paths });
                        }
                        cell_rects.push((idx, rect));
                        let painter = ui.painter();
                        if is_sel {
                            painter.rect_filled(
                                rect,
                                egui::CornerRadius::same(6),
                                super::theme::SELECTION,
                            );
                        } else if resp.hovered() {
                            painter.rect_filled(
                                rect,
                                egui::CornerRadius::same(6),
                                super::theme::HOVER,
                            );
                        }
                        let icon_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.center().x, rect.top() + 4.0 + ICON * 0.5),
                            egui::vec2(ICON, ICON),
                        );
                        if is_dir {
                            let peek_paths: Vec<PathBuf> =
                                folder_peek.peek(&path).to_vec();
                            let mut peek_tex = Vec::new();
                            for p in &peek_paths {
                                if let Some(t) = thumbs.get_or_load(ui.ctx(), p) {
                                    peek_tex.push(t);
                                }
                                if peek_tex.len() >= 4 {
                                    break;
                                }
                            }
                            if !peek_tex.is_empty() {
                                draw_folder_preview(painter, icon_rect, &peek_tex);
                            } else if let Some(tex) =
                                icons.icon_for(ui.ctx(), &path, true, false, 48)
                            {
                                painter.image(
                                    tex.id(),
                                    icon_rect,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            } else {
                                painter.text(
                                    icon_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "📁",
                                    egui::FontId::proportional(40.0),
                                    super::theme::TEXT,
                                );
                            }
                        } else if reparse {
                            if let Some(tex) =
                                icons.icon_for(ui.ctx(), &path, false, true, 48)
                            {
                                painter.image(
                                    tex.id(),
                                    icon_rect,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            } else {
                                painter.text(
                                    icon_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "🔗",
                                    egui::FontId::proportional(36.0),
                                    super::theme::TEXT,
                                );
                            }
                        } else if let Some(tex) = thumbs.get_or_load(ui.ctx(), &path) {
                            painter.image(
                                tex.id(),
                                icon_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        } else if let Some(tex) =
                            icons.icon_for(ui.ctx(), &path, false, false, 48)
                        {
                            painter.image(
                                tex.id(),
                                icon_rect,
                                egui::Rect::from_min_max(
                                    egui::Pos2::ZERO,
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        } else {
                            painter.text(
                                icon_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "📄",
                                egui::FontId::proportional(40.0),
                                super::theme::TEXT,
                            );
                        }
                        let name_top = rect.top() + 4.0 + ICON + 2.0;
                        let name_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + 3.0, name_top),
                            egui::vec2(CELL_W - 6.0, CELL_H - (name_top - rect.top()) - 3.0),
                        );
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            &name,
                            0.0,
                            egui::TextFormat {
                                font_id: egui::TextStyle::Body.resolve(ui.style()),
                                color: super::theme::TEXT,
                                ..Default::default()
                            },
                        );
                        job.wrap.max_width = name_rect.width();
                        job.wrap.max_rows = 1;
                        job.wrap.overflow_character = Some('…');
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        let gx = name_rect.center().x - galley.size().x * 0.5;
                        let gy = name_rect.center().y - galley.size().y * 0.5;
                        ui.painter().galley(egui::pos2(gx, gy), galley, super::theme::TEXT);
                        if resp.clicked() {
                            let input = ui.input(|i| {
                                (i.modifiers.ctrl || i.modifiers.command, i.modifiers.shift)
                            });
                            pane.select(idx, input.0, input.1);
                        }
                        if resp.double_clicked() {
                            if is_dir {
                                *row_action = Some(RowAction::Navigate(path));
                            } else {
                                *row_action = Some(RowAction::Open(path));
                            }
                        }
                        if resp.secondary_clicked() {
                            if !is_sel {
                                pane.select(idx, false, false);
                            }
                            if let Some(pos) = resp.interact_pointer_pos() {
                                *item_menu_fired = true;
                                events.push(PaneEvent::Activate(pane_idx));
                                events.push(PaneEvent::ContextMenu(
                                    pane_idx,
                                    pos,
                                    MenuTarget::Items(pane.selected_paths()),
                                ));
                            }
                        }
                    }
                });
            }
        });
}

fn apply_marquee(pane: &mut Pane, cell_rects: &[(usize, egui::Rect)], marquee: egui::Rect) {
    let ctrl = false;
    for &(idx, rect) in cell_rects {
        if marquee.intersects(rect) {
            pane.selected.insert(idx);
        }
    }
    let _ = ctrl;
    pane.anchor = cell_rects.first().map(|(i, _)| *i);
}

enum RowAction {
    Navigate(PathBuf),
    Open(PathBuf),
}

fn draw_folder_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    thumbs: &[egui::TextureHandle],
) {
    let tab_w = rect.width() * 0.4;
    let tab = egui::Rect::from_min_size(rect.min, egui::vec2(tab_w, 10.0));
    painter.rect_filled(
        tab,
        egui::CornerRadius {
            nw: 4,
            ne: 4,
            sw: 0,
            se: 0,
        },
        egui::Color32::from_rgb(0xFF, 0xD9, 0x66),
    );
    let body = egui::Rect::from_min_size(
        egui::pos2(rect.min.x, rect.min.y + 8.0),
        egui::vec2(rect.width(), rect.height() - 8.0),
    );
    painter.rect_filled(
        body,
        egui::CornerRadius::same(4),
        egui::Color32::from_rgb(0xFF, 0xE9, 0x9A),
    );
    let inner = body.shrink(4.0);
    let n = thumbs.len().min(4);
    let (cols, rows) = match n {
        1 => (1, 1),
        2 => (2, 1),
        _ => (2, 2),
    };
    let cw = inner.width() / cols as f32;
    let ch = inner.height() / rows as f32;
    for (i, t) in thumbs.iter().take(4).enumerate() {
        let c = i % cols;
        let r = i / cols;
        let cell = egui::Rect::from_min_size(
            egui::pos2(inner.min.x + c as f32 * cw, inner.min.y + r as f32 * ch),
            egui::vec2(cw - 1.0, ch - 1.0),
        );
        painter.image(
            t.id(),
            cell,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    painter.rect_stroke(
        body,
        4,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0xE0, 0xC0, 0x60)),
        egui::StrokeKind::Inside,
    );
}

/// 拖动中的半透明 ghost：跟随指针显示条目数 + 首个文件名。
pub fn render_drag_ghost(ctx: &egui::Context) {
    if !ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    let Some(payload) = egui::DragAndDrop::payload::<ItemDrag>(ctx) else {
        return;
    };
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let n = payload.paths.len();
    let first = payload
        .paths
        .first()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let text = if n <= 1 {
        first
    } else if i18n::en() {
        format!("{first} ({n} items)")
    } else {
        format!("{first}（共 {n} 项）")
    };
    egui::Area::new(egui::Id::new("vine_drag_ghost"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos + egui::vec2(16.0, 18.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(170))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_max_width(280.0);
                    ui.label(
                        egui::RichText::new(text)
                            .color(egui::Color32::from_white_alpha(230))
                            .size(12.0),
                    );
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_filter_cancels_same_parent_and_self() {
        let dest = PathBuf::from(r"D:\dst");
        let paths = vec![
            PathBuf::from(r"D:\dst\a.txt"),
            PathBuf::from(r"D:\src\b.txt"),
            PathBuf::from(r"D:\dst"),
        ];
        let out = filter_drop_paths(&paths, &dest);
        assert_eq!(out, vec![PathBuf::from(r"D:\src\b.txt")]);
    }

    #[test]
    fn drop_filter_all_cancelled_is_empty() {
        let dest = PathBuf::from(r"D:\dst");
        let paths = vec![
            PathBuf::from(r"D:\dst\a.txt"),
            PathBuf::from(r"D:\dst\b.txt"),
        ];
        let out = filter_drop_paths(&paths, &dest);
        assert!(out.is_empty());
    }

    #[test]
    fn drop_filter_normalizes_case_and_slash() {
        let dest = PathBuf::from(r"d:\dst\");
        let paths = vec![PathBuf::from(r"D:/DST"), PathBuf::from(r"D:\dst\child.txt")];
        let out = filter_drop_paths(&paths, &dest);
        assert!(out.is_empty());
    }

    #[test]
    fn pane_group_add_close_switch() {
        let t = tempfile::tempdir().unwrap();
        let mut g = PaneGroup::new(t.path().to_path_buf());
        assert_eq!(g.tabs.len(), 1);
        assert_eq!(g.active, 0);
        g.add_tab(t.path().to_path_buf());
        assert_eq!(g.tabs.len(), 2);
        assert_eq!(g.active, 1);
        g.active = 0;
        g.close_tab(0);
        assert_eq!(g.tabs.len(), 1);
        assert_eq!(g.active, 0);
    }

    #[test]
    fn pane_group_never_closes_last_tab() {
        let t = tempfile::tempdir().unwrap();
        let mut g = PaneGroup::new(t.path().to_path_buf());
        g.close_tab(0);
        assert_eq!(g.tabs.len(), 1);
    }

    #[test]
    fn tab_label_is_dir_name() {
        let t = tempfile::tempdir().unwrap();
        let g = PaneGroup::new(t.path().to_path_buf());
        let label = g.tab_label(0);
        assert!(!label.is_empty());
    }

    fn names(p: &Pane) -> Vec<String> {
        p.entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn sort_entries_dirs_first_all_keys() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("b.txt"), "bb").unwrap();
        std::fs::write(t.path().join("a.txt"), "a").unwrap();
        std::fs::create_dir(t.path().join("zdir")).unwrap();
        let mut p = Pane::new(t.path().to_path_buf());
        assert_eq!(names(&p), ["zdir", "a.txt", "b.txt"]);

        p.sort = SortKey::Size;
        p.sort_asc = false;
        p.sort_entries();
        assert_eq!(names(&p), ["zdir", "b.txt", "a.txt"]);

        p.sort = SortKey::Name;
        p.sort_asc = false;
        p.sort_entries();
        assert_eq!(names(&p), ["zdir", "b.txt", "a.txt"]);

        p.sort = SortKey::Name;
        p.sort_asc = true;
        p.sort_entries();
        assert_eq!(names(&p), ["zdir", "a.txt", "b.txt"]);
    }

    #[test]
    fn view_indices_filter_case_insensitive() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("Alpha.txt"), "1").unwrap();
        std::fs::write(t.path().join("beta.md"), "2").unwrap();
        let mut p = Pane::new(t.path().to_path_buf());
        assert_eq!(p.view_indices().len(), 2);
        p.filter = "ALP".into();
        let v = p.view_indices();
        assert_eq!(v.len(), 1);
        assert_eq!(p.entries[v[0]].name, "Alpha.txt");
        p.filter = "zzz".into();
        assert!(p.view_indices().is_empty());
    }

    #[test]
    fn select_all_respects_filter() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("keep.txt"), "1").unwrap();
        std::fs::write(t.path().join("skip.log"), "2").unwrap();
        let mut p = Pane::new(t.path().to_path_buf());
        p.filter = "keep".into();
        p.select_all();
        assert_eq!(p.selected.len(), 1);
        assert_eq!(
            p.selected_paths()[0].file_name().unwrap(),
            "keep.txt"
        );
        p.filter.clear();
        p.select_all();
        assert_eq!(p.selected.len(), 2);
    }

    #[test]
    fn reload_keeps_selection_by_name() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("one.txt"), "1").unwrap();
        std::fs::write(t.path().join("two.txt"), "2").unwrap();
        let mut p = Pane::new(t.path().to_path_buf());
        p.select(0, false, false);
        let sel_name = p.entries[p.selected.iter().next().copied().unwrap()]
            .name
            .clone();
        p.reload();
        assert_eq!(p.selected.len(), 1);
        let idx = p.selected.iter().next().copied().unwrap();
        assert_eq!(p.entries[idx].name, sel_name);
    }

    #[test]
    fn history_flags() {
        let t = tempfile::tempdir().unwrap();
        let sub = t.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let mut p = Pane::new(sub.clone());
        assert!(!p.can_back());
        assert!(!p.can_forward());
        assert!(p.can_up());
        p.navigate(t.path().to_path_buf());
        assert!(p.can_back());
        p.go_back();
        assert!(p.can_forward());
        assert_eq!(p.current, sub);
    }

    #[test]
    fn view_mode_defaults_to_list() {
        let t = tempfile::tempdir().unwrap();
        let p = Pane::new(t.path().to_path_buf());
        assert_eq!(p.view_mode, ViewMode::List);
    }

    #[test]
    fn sort_key_labels_nonempty() {
        for k in SortKey::ALL {
            assert!(!k.label().is_empty());
        }
    }
}
