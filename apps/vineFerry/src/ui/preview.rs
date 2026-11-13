use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_TEXT_BYTES: u64 = 256 * 1024;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u32 = 4_000_000;
const TEXT_SNIFF_BYTES: usize = 8192;
const TEXT_MAX_LINES: usize = 2000;

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "bmp", "webp"];

enum Content {
    Text {
        text: String,
        truncated: bool,
    },
    Image {
        texture: egui::TextureHandle,
        width: u32,
        height: u32,
    },
    Message(String),
}

pub struct PreviewPane {
    pub enabled: bool,
    content: Option<Content>,
    loaded_key: Option<(PathBuf, u64, u64)>,
}

impl PreviewPane {
    pub fn new() -> Self {
        PreviewPane {
            enabled: false,
            content: None,
            loaded_key: None,
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, selected: Option<&Path>) {
        if !self.enabled {
            return;
        }
        let Some(path) = selected else {
            if self.loaded_key.is_some() || self.content.is_none() {
                self.content = Some(Content::Message("选中单个文件以预览".into()));
                self.loaded_key = None;
            }
            return;
        };
        let meta = std::fs::metadata(path).ok();
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            self.content = Some(Content::Message("文件夹不支持预览".into()));
            self.loaded_key = Some((path.to_path_buf(), 0, 0));
            return;
        }
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(unix_secs)
            .unwrap_or(0);
        let key = (path.to_path_buf(), size, mtime);
        if self.loaded_key.as_ref() == Some(&key) {
            return;
        }
        self.content = Some(load_content(ctx, path, size));
        self.loaded_key = Some(key);
    }

    pub fn render(&mut self, ui: &mut egui::Ui) {
        ui.heading("预览");
        ui.separator();
        match &mut self.content {
            None => {
                ui.label(egui::RichText::new("选中单个文件以预览").weak());
            }
            Some(Content::Message(m)) => {
                ui.label(egui::RichText::new(m.as_str()).weak());
            }
            Some(Content::Text { text, truncated }) => {
                if *truncated {
                    ui.label(egui::RichText::new("内容过大，仅显示前段").weak());
                }
                egui::ScrollArea::vertical()
                    .id_salt("preview_text")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .frame(egui::Frame::NONE),
                        );
                    });
            }
            Some(Content::Image {
                texture,
                width,
                height,
            }) => {
                ui.label(format!("{width} × {height}"));
                let avail = ui.available_size() - egui::vec2(8.0, 28.0);
                let scale = (avail.x / *width as f32)
                    .min(avail.y / *height as f32)
                    .min(1.0);
                let disp = egui::vec2(*width as f32 * scale, *height as f32 * scale);
                let source = egui::load::SizedTexture::new(texture.id(), disp);
                ui.add(egui::Image::new(source).maintain_aspect_ratio(true));
            }
        }
    }
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn looks_like_text(buf: &[u8]) -> bool {
    !buf.contains(&0)
}

fn load_content(ctx: &egui::Context, path: &Path, size: u64) -> Content {
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return load_image(ctx, path, size);
    }
    if size == 0 {
        return Content::Message("空文件".into());
    }
    if size > MAX_TEXT_BYTES {
        return Content::Message("文件过大，不做文本预览".into());
    }
    let bytes = match std::fs::read(crate::core::paths::to_long_path(path)) {
        Ok(b) => b,
        Err(e) => return Content::Message(format!("读取失败：{e}")),
    };
    let sniff_end = bytes.len().min(TEXT_SNIFF_BYTES);
    if !looks_like_text(&bytes[..sniff_end]) {
        return Content::Message("二进制文件，不支持预览".into());
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut out = String::new();
    let mut truncated = false;
    for (i, line) in text.lines().enumerate() {
        if i >= TEXT_MAX_LINES {
            out.push_str("…（后续行已省略）");
            truncated = true;
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    Content::Text { text: out, truncated }
}

fn load_image(ctx: &egui::Context, path: &Path, size: u64) -> Content {
    if size > MAX_IMAGE_BYTES {
        return Content::Message("图片过大，不支持预览".into());
    }
    let reader = match image::ImageReader::open(crate::core::paths::to_long_path(path)) {
        Ok(r) => r,
        Err(e) => return Content::Message(format!("打开失败：{e}")),
    };
    let mut reader = match reader.with_guessed_format() {
        Ok(r) => r,
        Err(e) => return Content::Message(format!("识别格式失败：{e}")),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(MAX_IMAGE_BYTES);
    reader.limits(limits);
    let img = match reader.decode() {
        Ok(i) => i,
        Err(e) => return Content::Message(format!("解码失败：{e}")),
    };
    let img = if img.width().saturating_mul(img.height()) > MAX_IMAGE_PIXELS {
        let ratio = (MAX_IMAGE_PIXELS as f32 / (img.width() * img.height()) as f32).sqrt();
        let w = ((img.width() as f32 * ratio) as u32).max(1);
        let h = ((img.height() as f32 * ratio) as u32).max(1);
        img.resize(w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        rgba.as_raw(),
    );
    let texture = ctx.load_texture("preview", color, egui::TextureOptions::default());
    Content::Image {
        texture,
        width,
        height,
    }
}
