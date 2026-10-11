use super::bitmap::BitmapImage;
use super::dib::{decode_dib_to_rgba, encode_rgba_to_dib};

const CUR_TYPE: u16 = 2;

pub fn write_cur(images: &[BitmapImage]) -> Result<Vec<u8>, String> {
    if images.is_empty() {
        return Err("No images provided".into());
    }

    let count = images.len() as u16;
    let header_size = 6usize;
    let entry_size = 16usize;
    let entries_end = header_size + (entry_size * count as usize);

    let mut data_sections: Vec<Vec<u8>> = Vec::with_capacity(count as usize);

    for img in images {
        let (header_bytes, xor_data) = encode_rgba_to_dib(img);

        let row_bytes = ((img.width + 31) / 32) * 4;
        let and_mask_size = (row_bytes * img.height) as usize;

        let mut image_data = Vec::with_capacity(header_bytes.len() + xor_data.len() + and_mask_size);
        image_data.extend_from_slice(&header_bytes);
        image_data.extend_from_slice(&xor_data);

        let mut and_mask = vec![0u8; and_mask_size];
        for y in 0..img.height {
            for x in 0..img.width {
                let rgba = img.get_pixel(x, y);
                if rgba[3] == 0 {
                    let byte_idx = (y * row_bytes + x / 8) as usize;
                    and_mask[byte_idx] |= 1 << (7 - (x % 8));
                }
            }
        }
        image_data.extend_from_slice(&and_mask);

        data_sections.push(image_data);
    }

    let mut output = Vec::new();

    output.extend_from_slice(&0u16.to_le_bytes());
    output.extend_from_slice(&CUR_TYPE.to_le_bytes());
    output.extend_from_slice(&count.to_le_bytes());

    let mut data_offset = entries_end as u32;
    for (i, img) in images.iter().enumerate() {
        let w = if img.width > 255 { 0u8 } else { img.width as u8 };
        let h = if img.height > 255 { 0u8 } else { img.height as u8 };

        output.push(w);
        output.push(h);
        output.push(0);
        output.push(0);

        output.extend_from_slice(&(img.hotspot_x as u16).to_le_bytes());
        output.extend_from_slice(&(img.hotspot_y as u16).to_le_bytes());

        let size = data_sections[i].len() as u32;
        output.extend_from_slice(&size.to_le_bytes());
        output.extend_from_slice(&data_offset.to_le_bytes());

        data_offset += size;
    }

    for section in &data_sections {
        output.extend_from_slice(section);
    }

    Ok(output)
}

pub fn read_cur(data: &[u8]) -> Result<Vec<BitmapImage>, String> {
    if data.len() < 6 {
        return Err("File too short for CUR header".into());
    }

    let _reserved = u16::from_le_bytes([data[0], data[1]]);
    let file_type = u16::from_le_bytes([data[2], data[3]]);
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;

    if file_type != CUR_TYPE {
        return Err(format!("Not a CUR file: type={}", file_type));
    }

    let mut images = Vec::with_capacity(count);

    for i in 0..count {
        let entry_offset = 6 + i * 16;
        if entry_offset + 16 > data.len() {
            return Err("Entry offset out of bounds".into());
        }

        let _width = data[entry_offset];
        let _height = data[entry_offset + 1];
        let hotspot_x = u16::from_le_bytes([data[entry_offset + 4], data[entry_offset + 5]]) as u32;
        let hotspot_y = u16::from_le_bytes([data[entry_offset + 6], data[entry_offset + 7]]) as u32;
        let size = u32::from_le_bytes([
            data[entry_offset + 8],
            data[entry_offset + 9],
            data[entry_offset + 10],
            data[entry_offset + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            data[entry_offset + 12],
            data[entry_offset + 13],
            data[entry_offset + 14],
            data[entry_offset + 15],
        ]) as usize;

        if offset + size > data.len() {
            return Err("Image data out of bounds".into());
        }

        let image_data = &data[offset..offset + size];

        if image_data.len() < 40 {
            return Err("Image data too short for DIB header".into());
        }

        let bi_size = u32::from_le_bytes([
            image_data[0], image_data[1], image_data[2], image_data[3],
        ]) as usize;

        let header = &image_data[..bi_size];
        let _bi_bit_count = u16::from_le_bytes([header[14], header[15]]);
        let bi_height = i32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let actual_height = (bi_height / 2).unsigned_abs();
        let bi_width = i32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u32;

        let xor_size = (bi_width * actual_height * 4) as usize;
        let xor_start = bi_size;
        let xor_end = xor_start + xor_size;

        if xor_end > image_data.len() {
            return Err("XOR data out of bounds".into());
        }

        let xor_data = &image_data[xor_start..xor_end];

        let row_bytes = ((bi_width + 31) / 32 * 4) as usize;
        let and_size = row_bytes * actual_height as usize;
        let and_start = xor_end;
        let and_end = and_start + and_size;

        let and_data = if and_end <= image_data.len() {
            Some(&image_data[and_start..and_end])
        } else {
            None
        };

        let mut img = decode_dib_to_rgba(header, xor_data, and_data)?;
        img.hotspot_x = hotspot_x;
        img.hotspot_y = hotspot_y;

        images.push(img);
    }

    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::bitmap::BitmapImage;

    fn make_test_image() -> BitmapImage {
        let size = 32;
        let mut img = BitmapImage::new(size, size, 16, 16);
        for y in 0..size {
            for x in 0..size {
                if x < size / 2 {
                    img.set_pixel(x, y, [255, 0, 0, 255]);
                } else {
                    img.set_pixel(x, y, [0, 0, 255, 255]);
                }
            }
        }
        img
    }

    #[test]
    fn test_write_read_cur_roundtrip() {
        let img = make_test_image();
        let cur_data = write_cur(&[img.clone()]).unwrap();

        assert_eq!(cur_data[0..2], [0, 0]);
        assert_eq!(cur_data[2..4], 2u16.to_le_bytes());
        assert_eq!(cur_data[4..6], 1u16.to_le_bytes());

        let images = read_cur(&cur_data).unwrap();
        assert_eq!(images.len(), 1);

        let decoded = &images[0];
        assert_eq!(decoded.width, img.width);
        assert_eq!(decoded.height, img.height);
        assert_eq!(decoded.hotspot_x, 16);
        assert_eq!(decoded.hotspot_y, 16);
    }

    #[test]
    fn test_multi_size_cur() {
        let img16 = {
            let mut img = BitmapImage::new(16, 16, 8, 8);
            img.fill([255, 0, 0, 255]);
            img
        };
        let img32 = {
            let mut img = BitmapImage::new(32, 32, 16, 16);
            img.fill([0, 0, 255, 255]);
            img
        };
        let img48 = {
            let mut img = BitmapImage::new(48, 48, 24, 24);
            img.fill([0, 255, 0, 255]);
            img
        };

        let cur_data = write_cur(&[img16, img32, img48]).unwrap();
        let count = u16::from_le_bytes([cur_data[4], cur_data[5]]);
        assert_eq!(count, 3);

        let images = read_cur(&cur_data).unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].width, 16);
        assert_eq!(images[1].width, 32);
        assert_eq!(images[2].width, 48);
    }

    #[test]
    fn test_transparency() {
        let mut img = BitmapImage::new(8, 8, 0, 0);
        img.set_pixel(0, 0, [255, 255, 255, 255]);
        img.set_pixel(1, 1, [0, 0, 0, 0]);

        let cur_data = write_cur(&[img]).unwrap();
        let images = read_cur(&cur_data).unwrap();

        assert_eq!(images[0].get_pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(images[0].get_pixel(1, 1)[3], 0);
    }
}
