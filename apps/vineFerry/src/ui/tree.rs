use std::collections::{HashMap, HashSet};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::pane::{list_dir, PaneEvent};
use crate::ui::i18n;

pub struct NavTree {
    pub expanded: HashSet<PathBuf>,
    pub cache: HashMap<PathBuf, Vec<PathBuf>>,
}

impl NavTree {
    pub fn new() -> Self {
        NavTree {
            expanded: HashSet::new(),
            cache: HashMap::new(),
        }
    }

    fn dirs_of(&mut self, path: &Path) -> &[PathBuf] {
        if !self.cache.contains_key(path) {
            let mut dirs: Vec<PathBuf> = list_dir(path)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.is_dir && !e.reparse)
                .map(|e| path.join(e.name))
                .collect();
            dirs.sort();
            self.cache.insert(path.to_path_buf(), dirs);
        }
        self.cache.get(path).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn invalidate(&mut self) {
        self.cache.clear();
    }
}

pub fn default_bookmarks() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        for (label, sub) in [("桌面", "Desktop"), ("下载", "Downloads"), ("文档", "Documents")] {
            let p = Path::new(&profile).join(sub);
            if p.is_dir() {
                out.push((label.to_string(), p));
            }
        }
    }
    out
}

pub fn drive_list() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for c in b'A'..=b'Z' {
        let p = PathBuf::from(format!("{}:\\", c as char));
        if p.exists() {
            out.push(p);
        }
    }
    out
}

pub fn drive_display(path: &Path) -> String {
    let letter = path
        .to_string_lossy()
        .chars()
        .next()
        .unwrap_or('?')
        .to_ascii_uppercase();
    match volume_label(letter) {
        Some(l) if !l.is_empty() => format!("{l} ({letter}:)"),
        _ => format!("{} ({letter}:)", i18n::t("本地磁盘", "Local Disk")),
    }
}

fn volume_label(letter: char) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let root: Vec<u16> = std::ffi::OsStr::new(&format!("{letter}:\\"))
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut name = [0u16; 261];
    let mut fs = [0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            name.as_mut_ptr(),
            name.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let end = name.iter().position(|&c| c == 0)?;
    Some(String::from_utf16_lossy(&name[..end]))
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text).size(11.0).strong().weak());
    ui.add_space(4.0);
}

pub fn render_tree(
    ui: &mut egui::Ui,
    tree: &mut NavTree,
    current: &Path,
    pane_idx: usize,
) -> Vec<PaneEvent> {
    let mut events = Vec::new();

    section_label(ui, i18n::t("快速访问", "Quick Access"));
    for (label, path) in default_bookmarks() {
        let selected = paths_equal(current, &path);
        if ui.selectable_label(selected, format!("⭐ {label}")).clicked() {
            events.push(PaneEvent::Navigate(pane_idx, path));
        }
    }

    section_label(ui, i18n::t("此电脑", "This PC"));
    let drives = drive_list();
    for drive in drives {
        let label = drive_display(&drive);
        render_node(ui, tree, &drive, &label, current, &mut events, pane_idx, 0);
    }

    events
}

fn node_label(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().trim_end_matches('\\').to_string())
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches('\\'))
}

fn paint_chevron(ui: &egui::Ui, rect: egui::Rect, expanded: bool) {
    let color = ui.visuals().text_color().gamma_multiply(0.7);
    let c = rect.center();
    let s = (rect.width().min(rect.height()) * 0.32).max(3.0);
    let pts = if expanded {
        vec![
            egui::pos2(c.x - s, c.y - s * 0.55),
            egui::pos2(c.x + s, c.y - s * 0.55),
            egui::pos2(c.x, c.y + s),
        ]
    } else {
        vec![
            egui::pos2(c.x - s * 0.55, c.y - s),
            egui::pos2(c.x - s * 0.55, c.y + s),
            egui::pos2(c.x + s, c.y),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        pts,
        color,
        egui::Stroke::NONE,
    ));
}

#[allow(clippy::too_many_arguments)]
fn render_node(
    ui: &mut egui::Ui,
    tree: &mut NavTree,
    path: &Path,
    label: &str,
    current: &Path,
    events: &mut Vec<PaneEvent>,
    pane_idx: usize,
    depth: usize,
) {
    let is_expanded = tree.expanded.contains(path);
    let selected = paths_equal(current, path);

    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 14.0);
        let (chev_rect, chev_resp) =
            ui.allocate_exact_size(egui::vec2(16.0, 18.0), egui::Sense::click());
        if chev_resp.hovered() {
            ui.painter().rect_filled(
                chev_rect.expand(3.0),
                egui::CornerRadius::same(4),
                super::theme::HOVER,
            );
        }
        paint_chevron(ui, chev_rect, is_expanded);
        if chev_resp.clicked() && !tree.expanded.remove(path) {
            tree.expanded.insert(path.to_path_buf());
            tree.dirs_of(path);
        }
        chev_resp.on_hover_text(i18n::t("展开/收起", "Expand/Collapse"));
        if ui
            .selectable_label(selected, format!("📁 {label}"))
            .clicked()
        {
            events.push(PaneEvent::Navigate(pane_idx, path.to_path_buf()));
        }
    });

    if is_expanded {
        let children: Vec<PathBuf> = tree.dirs_of(path).to_vec();
        for child in children {
            let child_label = node_label(&child);
            render_node(
                ui,
                tree,
                &child,
                &child_label,
                current,
                events,
                pane_idx,
                depth + 1,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_display_shows_letter_suffix() {
        let d = drive_display(Path::new("C:\\"));
        assert!(d.ends_with("(C:)"), "got {d}");
    }

    #[test]
    fn node_label_falls_back_to_full_path() {
        let p = Path::new(r"C:\");
        assert!(!node_label(p).is_empty());
    }
}
