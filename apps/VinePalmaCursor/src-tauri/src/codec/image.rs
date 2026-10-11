use std::io::Cursor;

use image::{AnimationDecoder, ImageReader};

use super::bitmap::{BitmapImage, CursorFrame};

const MAX_DIM: u32 = 4096;

pub fn decode_image_frames(data: &[u8]) -> Result<Vec<CursorFrame>, String> {
    if data.starts_with(b"GIF8") {
        return decode_gif(data);
    }

    let img = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| format!("无法识别图片格式: {e}"))?
        .decode()
        .map_err(|e| format!("图片解码失败: {e}"))?;

    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    validate_dims(w, h)?;

    Ok(vec![CursorFrame {
        image: BitmapImage::from_rgba(w, h, w / 2, h / 2, rgba.into_raw()),
        delay_ms: 100,
    }])
}

fn decode_gif(data: &[u8]) -> Result<Vec<CursorFrame>, String> {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(data))
        .map_err(|e| format!("GIF 解码失败: {e}"))?;

    let frames_iter = decoder.into_frames();
    let mut out = Vec::new();

    for frame in frames_iter {
        let frame = frame.map_err(|e| format!("GIF 帧解码失败: {e}"))?;
        let delay_ms = frame.delay().numer_denom_ms().0;
        let rgba = frame.buffer().clone();
        let (w, h) = rgba.dimensions();
        validate_dims(w, h)?;

        out.push(CursorFrame {
            image: BitmapImage::from_rgba(w, h, w / 2, h / 2, rgba.into_raw()),
            delay_ms: if delay_ms == 0 { 100 } else { delay_ms },
        });
    }

    if out.is_empty() {
        return Err("GIF 中没有帧".into());
    }

    Ok(out)
}

fn validate_dims(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("图片尺寸无效".into());
    }
    if w > MAX_DIM || h > MAX_DIM {
        return Err(format!("图片过大: {}x{} (上限 {}x{})", w, h, MAX_DIM, MAX_DIM));
    }
    Ok(())
}