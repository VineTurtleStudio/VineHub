use eframe::egui::{self, Color32, CornerRadius, Stroke};

use crate::core::transfer::FileStatus;

pub fn accent() -> Color32 {
    Color32::from_rgb(59, 130, 246)
}

pub fn text_dim() -> Color32 {
    Color32::from_rgb(148, 163, 184)
}

fn line() -> Color32 {
    Color32::from_rgb(45, 52, 64)
}

pub fn status_color(status: FileStatus) -> Color32 {
    match status {
        FileStatus::Verified => Color32::from_rgb(46, 160, 67),
        FileStatus::Failed => Color32::from_rgb(220, 50, 47),
        FileStatus::Skipped => Color32::from_rgb(148, 163, 184),
        FileStatus::Cancelled => Color32::from_rgb(202, 138, 4),
    }
}

/// 全局深色主题：统一圆角 / 卡片底色 / 强调色
pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(Color32::from_rgb(226, 232, 240));
    v.panel_fill = Color32::from_rgb(17, 20, 27);
    v.window_fill = Color32::from_rgb(24, 28, 36);
    v.extreme_bg_color = Color32::from_rgb(13, 16, 22); // TextEdit 底色
    v.hyperlink_color = accent();
    v.selection.bg_fill = accent();
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    let radius = CornerRadius::same(6);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, line());
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(36, 42, 54);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, line());
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(226, 232, 240));
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(50, 58, 74);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(80, 92, 116));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(28, 33, 44);

    ctx.set_visuals(v);

    ctx.all_styles_mut(|style| {
        style.spacing.interact_size.y = 28.0;
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

/// 卡片容器：圆角 + 描边 + 浅底
pub fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(24, 28, 36))
        .stroke(Stroke::new(1.0, line()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add);
}
