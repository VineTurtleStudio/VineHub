use crate::codec::bitmap::{BitmapImage, CursorFrame};
use crate::codec::cur::write_cur;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportCurRequest {
    pub images: Vec<ImageData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorFrameData {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportAniRequest {
    pub frames: Vec<CursorFrameData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub format: String,
    pub frames: Vec<CursorFrameData>,
}

#[tauri::command]
pub fn export_cur(request: ExportCurRequest) -> Result<Vec<u8>, String> {
    let mut images = Vec::with_capacity(request.images.len());

    for img_data in &request.images {
        let img = BitmapImage::from_rgba(
            img_data.width,
            img_data.height,
            img_data.hotspot_x,
            img_data.hotspot_y,
            img_data.rgba.clone(),
        );
        images.push(img);
    }

    write_cur(&images)
}

#[tauri::command]
pub fn export_ani(request: ExportAniRequest) -> Result<Vec<u8>, String> {
    let frames: Vec<CursorFrame> = request
        .frames
        .into_iter()
        .map(|f| CursorFrame {
            image: BitmapImage::from_rgba(
                f.width,
                f.height,
                f.hotspot_x,
                f.hotspot_y,
                f.rgba,
            ),
            delay_ms: f.delay_ms,
        })
        .collect();

    crate::codec::ani::write_ani(&frames)
}

#[tauri::command]
pub fn import_cursor(data: Vec<u8>) -> Result<Vec<ImageData>, String> {
    let images = crate::codec::cur::read_cur(&data)?;

    Ok(images
        .into_iter()
        .map(|img| ImageData {
            width: img.width,
            height: img.height,
            hotspot_x: img.hotspot_x,
            hotspot_y: img.hotspot_y,
            rgba: img.rgba_data,
        })
        .collect())
}

#[tauri::command]
pub fn import_file(data: Vec<u8>) -> Result<ImportResult, String> {
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"ACON" {
        let frames = crate::codec::ani::read_ani(&data)?;
        return Ok(ImportResult {
            format: "ani".into(),
            frames: frames.into_iter().map(frame_to_data).collect(),
        });
    }

    if data.len() >= 4 && data[0] == 0 && data[1] == 0 && data[2] == 2 && data[3] == 0 {
        let images = crate::codec::cur::read_cur(&data)?;
        return Ok(ImportResult {
            format: "cur".into(),
            frames: images
                .into_iter()
                .map(to_empty_frame)
                .collect(),
        });
    }

    let frames = crate::codec::image::decode_image_frames(&data)?;
    let format = detect_image_format(&data).unwrap_or_else(|| "image".into());
    Ok(ImportResult {
        format,
        frames: frames.into_iter().map(frame_to_data).collect(),
    })
}

fn frame_to_data(frame: CursorFrame) -> CursorFrameData {
    CursorFrameData {
        width: frame.image.width,
        height: frame.image.height,
        hotspot_x: frame.image.hotspot_x,
        hotspot_y: frame.image.hotspot_y,
        rgba: frame.image.rgba_data,
        delay_ms: frame.delay_ms,
    }
}

fn to_empty_frame(img: BitmapImage) -> CursorFrameData {
    CursorFrameData {
        width: img.width,
        height: img.height,
        hotspot_x: img.hotspot_x,
        hotspot_y: img.hotspot_y,
        rgba: img.rgba_data,
        delay_ms: 100,
    }
}

fn detect_image_format(data: &[u8]) -> Option<String> {
    if data.starts_with(b"\x89PNG") {
        Some("png".into())
    } else if data.starts_with(b"GIF8") {
        Some("gif".into())
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("jpeg".into())
    } else if data.starts_with(b"BM") {
        Some("bmp".into())
    } else {
        None
    }
}