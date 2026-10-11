use super::bitmap::BitmapImage;

const BITMAPINFOHEADER_SIZE: u32 = 40;

#[derive(Debug, Clone)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

impl BitmapInfoHeader {
    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(BITMAPINFOHEADER_SIZE as usize);
        buf.extend_from_slice(&self.bi_size.to_le_bytes());
        buf.extend_from_slice(&self.bi_width.to_le_bytes());
        buf.extend_from_slice(&self.bi_height.to_le_bytes());
        buf.extend_from_slice(&self.bi_planes.to_le_bytes());
        buf.extend_from_slice(&self.bi_bit_count.to_le_bytes());
        buf.extend_from_slice(&self.bi_compression.to_le_bytes());
        buf.extend_from_slice(&self.bi_size_image.to_le_bytes());
        buf.extend_from_slice(&self.bi_x_pels_per_meter.to_le_bytes());
        buf.extend_from_slice(&self.bi_y_pels_per_meter.to_le_bytes());
        buf.extend_from_slice(&self.bi_clr_used.to_le_bytes());
        buf.extend_from_slice(&self.bi_clr_important.to_le_bytes());
        buf
    }

    fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < BITMAPINFOHEADER_SIZE as usize {
            return Err("DIB header too short".into());
        }
        Ok(Self {
            bi_size: read_u32_le(data, 0),
            bi_width: read_i32_le(data, 4),
            bi_height: read_i32_le(data, 8),
            bi_planes: read_u16_le(data, 12),
            bi_bit_count: read_u16_le(data, 14),
            bi_compression: read_u32_le(data, 16),
            bi_size_image: read_u32_le(data, 20),
            bi_x_pels_per_meter: read_i32_le(data, 24),
            bi_y_pels_per_meter: read_i32_le(data, 28),
            bi_clr_used: read_u32_le(data, 32),
            bi_clr_important: read_u32_le(data, 36),
        })
    }
}

pub fn encode_rgba_to_dib(img: &BitmapImage) -> (Vec<u8>, Vec<u8>) {
    let width = img.width as i32;
    let height = img.height as i32;

    let header = BitmapInfoHeader {
        bi_size: BITMAPINFOHEADER_SIZE,
        bi_width: width,
        bi_height: height * 2,
        bi_planes: 1,
        bi_bit_count: 32,
        bi_compression: 0,
        bi_size_image: (img.width * img.height * 4),
        bi_x_pels_per_meter: 0,
        bi_y_pels_per_meter: 0,
        bi_clr_used: 0,
        bi_clr_important: 0,
    };

    let header_bytes = header.to_bytes();

    let mut xor_data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..img.height {
        for x in 0..img.width {
            let rgba = img.get_pixel(x, y);
            xor_data.push(rgba[2]);
            xor_data.push(rgba[1]);
            xor_data.push(rgba[0]);
            xor_data.push(rgba[3]);
        }
    }

    (header_bytes, xor_data)
}

pub fn decode_dib_to_rgba(
    header_data: &[u8],
    xor_data: &[u8],
    and_data: Option<&[u8]>,
) -> Result<BitmapImage, String> {
    let header = BitmapInfoHeader::from_bytes(header_data)?;

    if header.bi_bit_count != 32 {
        return Err(format!("Unsupported bit count: {}", header.bi_bit_count));
    }

    let width = header.bi_width as u32;
    let height = (header.bi_height / 2).unsigned_abs();

    let mut rgba_data = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            if idx + 3 >= xor_data.len() {
                return Err("XOR data too short".into());
            }
            let b = xor_data[idx];
            let g = xor_data[idx + 1];
            let r = xor_data[idx + 2];
            let a = xor_data[idx + 3];
            rgba_data.push(r);
            rgba_data.push(g);
            rgba_data.push(b);
            rgba_data.push(a);
        }
    }

    if let Some(and) = and_data {
        let row_bytes = ((width + 31) / 32) * 4;
        for y in 0..height {
            for x in 0..width {
                let byte_idx = (y * row_bytes + x / 8) as usize;
                if byte_idx < and.len() {
                    let bit = (and[byte_idx] >> (7 - (x % 8))) & 1;
                    if bit == 1 {
                        let idx = ((y * width + x) * 4 + 3) as usize;
                        rgba_data[idx] = 0;
                    }
                }
            }
        }
    }

    Ok(BitmapImage::from_rgba(width, height, 0, 0, rgba_data))
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn read_i32_le(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}
