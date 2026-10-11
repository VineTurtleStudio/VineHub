use super::bitmap::{BitmapImage, CursorFrame};
use super::cur::{read_cur, write_cur};

const JIFFIES_PER_SECOND: f64 = 60.0;

pub fn ms_to_jiffies(ms: u32) -> u32 {
    let j = ((ms as f64) * JIFFIES_PER_SECOND / 1000.0).round() as u32;
    j.max(1)
}

pub fn jiffies_to_ms(jiffies: u32) -> u32 {
    ((jiffies as f64) * 1000.0 / JIFFIES_PER_SECOND).round() as u32
}

pub fn write_ani(frames: &[CursorFrame]) -> Result<Vec<u8>, String> {
    if frames.is_empty() {
        return Err("No frames provided".into());
    }

    let n = frames.len() as u32;
    let width = frames[0].image.width;
    let height = frames[0].image.height;

    for frame in frames {
        if frame.image.width != width || frame.image.height != height {
            return Err(format!(
                "All ANI frames must have the same size, got {}x{} and {}x{}",
                width, height, frame.image.width, frame.image.height
            ));
        }
    }

    let default_jiffies = ms_to_jiffies(frames[0].delay_ms);

    let mut output: Vec<u8> = Vec::new();
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&0u32.to_le_bytes());
    output.extend_from_slice(b"ACON");

    let header = AniHeader {
        n_frames: n,
        n_steps: n,
        i_width: width,
        i_height: height,
        n_bit_count: 32,
        n_planes: 1,
        i_disp_rate: default_jiffies,
        b_attributes: 0,
    };

    write_chunk(&mut output, b"anih", &header.to_bytes());

    let mut rates: Vec<u8> = Vec::with_capacity(frames.len() * 4);
    for frame in frames {
        rates.extend_from_slice(&ms_to_jiffies(frame.delay_ms).to_le_bytes());
    }
    write_chunk(&mut output, b"rate", &rates);

    let mut list_data: Vec<u8> = Vec::new();
    list_data.extend_from_slice(b"fram");
    for frame in frames {
        let icon_bytes = write_cur(&[frame.image.clone()])?;
        write_chunk(&mut list_data, b"icon", &icon_bytes);
    }
    write_chunk(&mut output, b"LIST", &list_data);

    let riff_size = output.len() as u32 - 8;
    output[4..8].copy_from_slice(&riff_size.to_le_bytes());

    Ok(output)
}

fn write_chunk(output: &mut Vec<u8>, fourcc: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(fourcc);
    output.extend_from_slice(&(data.len() as u32).to_le_bytes());
    output.extend_from_slice(data);
    if data.len() % 2 == 1 {
        output.push(0);
    }
}

#[derive(Debug, Clone, Copy)]
struct AniHeader {
    n_frames: u32,
    n_steps: u32,
    i_width: u32,
    i_height: u32,
    n_bit_count: u32,
    n_planes: u32,
    i_disp_rate: u32,
    b_attributes: u32,
}

impl AniHeader {
    const SIZE: usize = 36;

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE);
        buf.extend_from_slice(&(Self::SIZE as u32).to_le_bytes());
        buf.extend_from_slice(&self.n_frames.to_le_bytes());
        buf.extend_from_slice(&self.n_steps.to_le_bytes());
        buf.extend_from_slice(&self.i_width.to_le_bytes());
        buf.extend_from_slice(&self.i_height.to_le_bytes());
        buf.extend_from_slice(&self.n_bit_count.to_le_bytes());
        buf.extend_from_slice(&self.n_planes.to_le_bytes());
        buf.extend_from_slice(&self.i_disp_rate.to_le_bytes());
        buf.extend_from_slice(&self.b_attributes.to_le_bytes());
        buf
    }

    fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < Self::SIZE {
            return Err("anih chunk too short".into());
        }
        Ok(Self {
            n_frames: read_u32_le(data, 4),
            n_steps: read_u32_le(data, 8),
            i_width: read_u32_le(data, 12),
            i_height: read_u32_le(data, 16),
            n_bit_count: read_u32_le(data, 20),
            n_planes: read_u32_le(data, 24),
            i_disp_rate: read_u32_le(data, 28),
            b_attributes: read_u32_le(data, 32),
        })
    }
}

pub fn read_ani(data: &[u8]) -> Result<Vec<CursorFrame>, String> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"ACON" {
        return Err("Not a valid ANI file (RIFF/ACON expected)".into());
    }

    let mut header: Option<AniHeader> = None;
    let mut rates: Vec<u32> = Vec::new();
    let mut icons: Vec<BitmapImage> = Vec::new();
    let mut seq: Option<Vec<u16>> = None;

    let mut offset = 12usize;
    while offset + 8 <= data.len() {
        if data[offset..offset + 4] == *b"LIST" {
            let size = read_u32_le(data, offset + 4) as usize;
            let body_start = offset + 8;
            let body_end = (body_start + size).min(data.len());
            if body_start + 4 <= body_end && &data[body_start..body_start + 4] == b"fram" {
                let mut inner = body_start + 4;
                while inner + 8 <= body_end {
                    let inner_size = read_u32_le(data, inner + 4) as usize;
                    let chunk_body = inner + 8;
                    if &data[inner..inner + 4] == b"icon" {
                        let icon_end = (chunk_body + inner_size).min(body_end);
                        let icon_data = &data[chunk_body..icon_end];
                        let mut imgs = read_cur(icon_data)?;
                        if !imgs.is_empty() {
                            icons.push(imgs.remove(0));
                        }
                    }
                    inner = chunk_body + inner_size;
                    if inner_size % 2 == 1 {
                        inner += 1;
                    }
                }
            }
            offset = body_start + size;
            if size % 2 == 1 {
                offset += 1;
            }
            continue;
        }

        let id = data[offset..offset + 4].to_vec();
        let size = read_u32_le(data, offset + 4) as usize;
        let chunk = offset + 8;
        let chunk_end = (chunk + size).min(data.len());

        match id.as_slice() {
            b"anih" => {
                header = Some(AniHeader::from_bytes(&data[chunk..chunk_end])?);
            }
            b"rate" => {
                rates = read_u32_array(&data[chunk..chunk_end]);
            }
            b"seq" => {
                let mut s = Vec::with_capacity(size / 2);
                for i in 0..(chunk_end - chunk) / 2 {
                    s.push(read_u16_le(data, chunk + i * 2));
                }
                seq = Some(s);
            }
            _ => {}
        }

        offset = chunk + size;
        if size % 2 == 1 {
            offset += 1;
        }
    }

    let header = header.ok_or("ANIH chunk missing")?;
    if icons.is_empty() {
        return Err("ANI contains no frames".into());
    }

    let order: Vec<usize> = match &seq {
        Some(s) if s.len() >= icons.len() => s.iter().map(|&i| i as usize).take(icons.len()).collect(),
        _ => (0..icons.len()).collect(),
    };

    let ordered_icons: Vec<&BitmapImage> = order.iter().map(|&i| &icons[i]).collect();

    let default_jiffies = header.i_disp_rate;

    let frames = ordered_icons
        .into_iter()
        .enumerate()
        .map(|(i, img)| {
            let jiffies = rates.get(i).copied().unwrap_or(default_jiffies);
            CursorFrame {
                image: img.clone(),
                delay_ms: jiffies_to_ms(jiffies),
            }
        })
        .collect();

    Ok(frames)
}

fn read_u32_array(data: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(data.len() / 4);
    for i in 0..data.len() / 4 {
        out.push(read_u32_le(data, i * 4));
    }
    out
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}