use std::collections::HashMap;
use std::path::{Path, PathBuf};

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "bmp", "webp"];
const MAX_ENTRIES: usize = 768;
const FRAME_BUDGET: u8 = 8;
const PEEK_MAX: usize = 4;

pub struct FolderPeek {
    map: HashMap<PathBuf, Vec<PathBuf>>,
    budget: u8,
}

impl FolderPeek {
    pub fn new() -> Self {
        FolderPeek {
            map: HashMap::new(),
            budget: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.budget = 4;
    }

    pub fn peek(&mut self, folder: &Path) -> &[PathBuf] {
        if self.map.contains_key(folder) {
            return self.map.get(folder).map(|v| v.as_slice()).unwrap_or(&[]);
        }
        if self.budget == 0 {
            return &[];
        }
        self.budget -= 1;
        let mut imgs = Vec::new();
        if let Ok(rd) = std::fs::read_dir(crate::core::paths::to_long_path(folder)) {
            let mut names: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .map(|x| IMAGE_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str()))
                        .unwrap_or(false)
                })
                .collect();
            names.sort();
            names.truncate(PEEK_MAX);
            imgs = names;
        }
        self.map.insert(folder.to_path_buf(), imgs);
        self.map.get(folder).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

pub struct ThumbCache {
    map: HashMap<PathBuf, Option<egui::TextureHandle>>,
    order: Vec<PathBuf>,
    budget: u8,
}

impl ThumbCache {
    pub fn new() -> Self {
        ThumbCache {
            map: HashMap::new(),
            order: Vec::new(),
            budget: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.budget = FRAME_BUDGET;
    }

    pub fn get_or_load(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Option<egui::TextureHandle> {
        let ext_ok = path
            .extension()
            .map(|e| IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
            .unwrap_or(false);
        if !ext_ok {
            return None;
        }
        if let Some(v) = self.map.get(path) {
            return v.clone();
        }
        if self.budget == 0 {
            return None;
        }
        self.budget -= 1;
        let tex = load_thumb(ctx, path);
        if self.order.len() >= MAX_ENTRIES
            && let Some(old) = self.order.first().cloned()
        {
            self.order.remove(0);
            self.map.remove(&old);
        }
        self.map.insert(path.to_path_buf(), tex.clone());
        self.order.push(path.to_path_buf());
        tex
    }
}

fn load_thumb(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    let reader = image::ImageReader::open(crate::core::paths::to_long_path(path)).ok()?;
    let mut reader = reader.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let thumb = img.thumbnail(96, 96);
    let rgba = thumb.to_rgba8();
    let (w, h) = rgba.dimensions();
    let color =
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
    Some(ctx.load_texture(
        "thumb",
        color,
        egui::TextureOptions::LINEAR,
    ))
}
