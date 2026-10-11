use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitmapImage {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub rgba_data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorFrame {
    pub image: BitmapImage,
    pub delay_ms: u32,
}

impl BitmapImage {
    pub fn new(width: u32, height: u32, hotspot_x: u32, hotspot_y: u32) -> Self {
        let size = (width * height * 4) as usize;
        Self {
            width,
            height,
            hotspot_x,
            hotspot_y,
            rgba_data: vec![0u8; size],
        }
    }

    pub fn from_rgba(width: u32, height: u32, hotspot_x: u32, hotspot_y: u32, data: Vec<u8>) -> Self {
        let expected = (width * height * 4) as usize;
        assert_eq!(data.len(), expected, "RGBA data length mismatch: expected {}, got {}", expected, data.len());
        Self {
            width,
            height,
            hotspot_x,
            hotspot_y,
            rgba_data: data,
        }
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * self.width + x) * 4) as usize;
        [
            self.rgba_data[offset],
            self.rgba_data[offset + 1],
            self.rgba_data[offset + 2],
            self.rgba_data[offset + 3],
        ]
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let offset = ((y * self.width + x) * 4) as usize;
        self.rgba_data[offset] = rgba[0];
        self.rgba_data[offset + 1] = rgba[1];
        self.rgba_data[offset + 2] = rgba[2];
        self.rgba_data[offset + 3] = rgba[3];
    }

    pub fn pixel_count(&self) -> u32 {
        self.width * self.height
    }

    pub fn clear(&mut self) {
        self.rgba_data.fill(0);
    }

    pub fn fill(&mut self, rgba: [u8; 4]) {
        for i in (0..self.rgba_data.len()).step_by(4) {
            self.rgba_data[i] = rgba[0];
            self.rgba_data[i + 1] = rgba[1];
            self.rgba_data[i + 2] = rgba[2];
            self.rgba_data[i + 3] = rgba[3];
        }
    }
}
