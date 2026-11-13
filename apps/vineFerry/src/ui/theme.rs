use egui::{Color32, CornerRadius, Margin, Stroke};

pub const CHROME: Color32 = Color32::from_rgb(0xF2, 0xF5, 0xF8);
pub const BORDER: Color32 = Color32::from_rgb(0xDE, 0xE3, 0xE9);
pub const HEADER_BG: Color32 = Color32::from_rgb(0xF5, 0xF7, 0xFA);
pub const TEXT: Color32 = Color32::from_rgb(0x1B, 0x1F, 0x23);
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0x78, 0xD4);
pub const SELECTION: Color32 = Color32::from_rgb(0xCC, 0xE4, 0xF7);
pub const HOVER: Color32 = Color32::from_rgb(0xEC, 0xF1, 0xF6);

pub const TAB_TOP: CornerRadius = CornerRadius {
    nw: 6,
    ne: 6,
    sw: 0,
    se: 0,
};

pub fn bar_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CHROME)
        .inner_margin(Margin::symmetric(6, 4))
}

pub fn tab_strip_frame() -> egui::Frame {
    egui::Frame::new().fill(CHROME).inner_margin(Margin {
        left: 6,
        right: 6,
        top: 3,
        bottom: 0,
    })
}

pub fn flat_buttons(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets.inactive;
    w.weak_bg_fill = Color32::TRANSPARENT;
    w.bg_stroke = Stroke::NONE;
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.override_text_color = Some(TEXT);
    v.weak_text_alpha = 0.62;
    v.panel_fill = Color32::WHITE;
    v.window_fill = Color32::WHITE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.hyperlink_color = Color32::from_rgb(0x00, 0x67, 0xC0);
    v.faint_bg_color = Color32::from_rgb(0xF7, 0xF9, 0xFB);
    v.extreme_bg_color = Color32::WHITE;
    v.code_bg_color = Color32::from_rgb(0xF3, 0xF5, 0xF7);
    v.selection = egui::style::Selection {
        bg_fill: SELECTION,
        stroke: Stroke::new(1.0, TEXT),
    };
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
    v.button_frame = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::WHITE;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = CornerRadius::same(4);

    w.inactive.weak_bg_fill = Color32::WHITE;
    w.inactive.bg_fill = Color32::WHITE;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = CornerRadius::same(4);

    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xC5, 0xCE, 0xD8));
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.corner_radius = CornerRadius::same(4);

    w.active.weak_bg_fill = Color32::from_rgb(0xE2, 0xE8, 0xEF);
    w.active.bg_fill = Color32::from_rgb(0xE2, 0xE8, 0xEF);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.corner_radius = CornerRadius::same(4);

    w.open.weak_bg_fill = HOVER;
    w.open.bg_fill = HOVER;
    w.open.bg_stroke = Stroke::new(1.0, BORDER);
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.corner_radius = CornerRadius::same(4);

    v
}

pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size = egui::vec2(24.0, 24.0);
        style.spacing.indent = 14.0;
    });
}
