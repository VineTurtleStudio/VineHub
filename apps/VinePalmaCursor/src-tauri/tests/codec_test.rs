use vine_palma_cursor_lib::codec::ani::{ms_to_jiffies, read_ani, write_ani, jiffies_to_ms};
use vine_palma_cursor_lib::codec::bitmap::{BitmapImage, CursorFrame};
use vine_palma_cursor_lib::codec::cur::{read_cur, write_cur};
use vine_palma_cursor_lib::codec::dib::{decode_dib_to_rgba, encode_rgba_to_dib};
use vine_palma_cursor_lib::codec::image::decode_image_frames;
use vine_palma_cursor_lib::codec::scale::{downsample, generate_sizes, InterpolationMode};

#[test]
fn png_to_cur_minimal() {
    let mut img = BitmapImage::new(32, 32, 16, 16);

    const CHECKER: u32 = 4;
    for y in 0..32u32 {
        for x in 0..32u32 {
            if (x / CHECKER + y / CHECKER) % 2 == 0 {
                img.set_pixel(x, y, [0, 0, 0, 255]);
            }
        }
    }

    let cur = write_cur(&[img.clone()]).expect("write_cur");

    assert_eq!(&cur[0..2], &[0, 0], "reserved must be zero");
    assert_eq!(&cur[2..4], &[2, 0], "type must be CUR(2)");
    assert_eq!(&cur[4..6], &[1, 0], "must contain exactly 1 image");

    let decoded = read_cur(&cur).expect("read_cur");
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].width, 32);
    assert_eq!(decoded[0].height, 32);
    assert_eq!(decoded[0].hotspot_x, 16);
    assert_eq!(decoded[0].hotspot_y, 16);

    for y in 0..32u32 {
        for x in 0..32u32 {
            assert_eq!(decoded[0].get_pixel(x, y), img.get_pixel(x, y));
        }
    }
}

#[test]
fn multi_size_cur() {
    let mut images = Vec::new();
    for &size in &[16u32, 32, 48] {
        let mut img = BitmapImage::new(size, size, size / 2, size / 2);
        img.fill([120, 30, 200, 255]);
        images.push(img);
    }

    let cur = write_cur(&images).expect("write_cur multi");
    let decoded = read_cur(&cur).expect("read_cur multi");

    assert_eq!(decoded.len(), 3);
    let widths: Vec<u32> = decoded.iter().map(|i| i.width).collect();
    assert_eq!(widths, vec![16, 32, 48]);
    let hotspots: Vec<(u32, u32)> = decoded.iter().map(|i| (i.hotspot_x, i.hotspot_y)).collect();
    assert_eq!(hotspots, vec![(8, 8), (16, 16), (24, 24)]);
}

#[test]
fn dib_roundtrip_alpha() {
    let mut img = BitmapImage::new(8, 8, 0, 0);
    img.set_pixel(0, 0, [255, 0, 0, 128]);
    img.set_pixel(3, 4, [0, 255, 0, 64]);
    img.set_pixel(7, 7, [0, 0, 255, 255]);
    img.set_pixel(1, 1, [0, 0, 0, 0]);

    let (header, xor) = encode_rgba_to_dib(&img);
    let decoded = decode_dib_to_rgba(&header, &xor, None).expect("decode");

    assert_eq!(decoded.width, 8);
    assert_eq!(decoded.height, 8);
    assert_eq!(decoded.get_pixel(0, 0), [255, 0, 0, 128]);
    assert_eq!(decoded.get_pixel(3, 4), [0, 255, 0, 64]);
    assert_eq!(decoded.get_pixel(7, 7), [0, 0, 255, 255]);
}

#[test]
fn downsample_preserves_hotspot_ratio() {
    let mut img = BitmapImage::new(64, 64, 48, 8);
    img.fill([9, 9, 9, 255]);

    let small = downsample(&img, 32, 32, InterpolationMode::NearestNeighbor);
    assert_eq!(small.hotspot_x, 24);
    assert_eq!(small.hotspot_y, 4);
    assert_eq!(small.width, 32);
    assert_eq!(small.height, 32);
}

#[test]
fn generate_standard_sizes() {
    let sizes = generate_sizes(96);
    assert!(sizes.contains(&16));
    assert!(sizes.contains(&24));
    assert!(sizes.contains(&32));
    assert!(sizes.contains(&48));
    assert!(sizes.contains(&64));
    assert!(sizes.contains(&96));
    assert!(!sizes.contains(&128));
    assert_eq!(sizes[0], 16);
}

fn frame_with(delay_ms: u32, fill: [u8; 4]) -> CursorFrame {
    let mut img = BitmapImage::new(16, 16, 8, 8);
    img.fill(fill);
    CursorFrame { image: img, delay_ms }
}

#[test]
fn ani_write_read_roundtrip() {
    let frames = vec![
        frame_with(100, [255, 0, 0, 255]),
        frame_with(200, [0, 255, 0, 255]),
        frame_with(50, [0, 0, 255, 255]),
    ];

    let ani = write_ani(&frames).expect("write_ani");

    assert_eq!(&ani[0..4], b"RIFF");
    assert_eq!(&ani[8..12], b"ACON");

    let decoded = read_ani(&ani).expect("read_ani");
    assert_eq!(decoded.len(), 3);
    for (i, d) in decoded.iter().enumerate() {
        assert_eq!(d.image.width, 16);
        assert_eq!(d.image.height, 16);
        assert_eq!(d.image.hotspot_x, 8);
        assert_eq!(d.image.hotspot_y, 8);
        assert_eq!(d.delay_ms, frames[i].delay_ms, "frame {} delay", i);
        assert_eq!(d.image.get_pixel(0, 0), frames[i].image.get_pixel(0, 0));
    }
}

#[test]
fn ani_rejects_mixed_sizes() {
    let mut small = BitmapImage::new(16, 16, 8, 8);
    small.fill([255, 255, 255, 255]);
    let mut big = BitmapImage::new(32, 32, 16, 16);
    big.fill([0, 0, 0, 255]);

    let frames = vec![
        CursorFrame { image: small, delay_ms: 100 },
        CursorFrame { image: big, delay_ms: 100 },
    ];

    assert!(write_ani(&frames).is_err(), "mixed-size ANI should error");
}

#[test]
fn ani_jiffies_conversion() {
    assert_eq!(ms_to_jiffies(1000), 60);
    assert_eq!(ms_to_jiffies(100), 6);
    assert_eq!(ms_to_jiffies(0), 1);
    assert_eq!(jiffies_to_ms(60), 1000);
    assert_eq!(jiffies_to_ms(6), 100);
}

#[test]
fn gif_frames_decode() {
    let mut data = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut data);
        let frame1 = image::RgbaImage::from_pixel(16, 16, image::Rgba([255u8, 0, 0, 255]));
        let frame2 = image::RgbaImage::from_pixel(16, 16, image::Rgba([0u8, 0, 255, 255]));

        encoder
            .encode_frames(vec![
                image::Frame::from_parts(frame1.clone(), 0, 0, image::Delay::from_numer_denom_ms(100, 1)),
                image::Frame::from_parts(frame2.clone(), 0, 0, image::Delay::from_numer_denom_ms(250, 1)),
            ])
            .expect("encode gif");
    }

    assert!(!data.is_empty(), "GIF encoder should write data");

    let frames = decode_image_frames(&data).expect("decode gif");
    assert_eq!(frames.len(), 2, "GIF should split into 2 frames");
    assert_eq!(frames[0].delay_ms, 100);
    assert_eq!(frames[1].delay_ms, 250);
    assert_eq!(frames[0].image.get_pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(frames[1].image.get_pixel(0, 0), [0, 0, 255, 255]);
}

#[test]
fn png_single_frame_decode() {
    use image::ImageEncoder;

    let mut png_data = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
        let frame = image::RgbaImage::from_pixel(8, 8, image::Rgba([10u8, 20, 30, 255]));
        encoder
            .write_image(frame.as_raw(), 8, 8, image::ExtendedColorType::Rgba8)
            .expect("encode png");
    }

    let frames = decode_image_frames(&png_data).expect("decode png");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].image.width, 8);
    assert_eq!(frames[0].image.height, 8);
    assert_eq!(frames[0].image.get_pixel(0, 0), [10, 20, 30, 255]);
}

#[test]
fn import_file_auto_detects_formats() {
    use vine_palma_cursor_lib::commands::{
        export_ani, export_cur, import_file, CursorFrameData, ExportAniRequest, ImageData,
    };

    let rgba = {
        let mut px = Vec::with_capacity(8 * 8 * 4);
        for _ in 0..8 * 8 {
            px.push(200);
            px.push(100);
            px.push(50);
            px.push(255);
        }
        px
    };

    let ani = export_ani(ExportAniRequest {
        frames: vec![CursorFrameData {
            width: 8,
            height: 8,
            hotspot_x: 4,
            hotspot_y: 4,
            rgba: rgba.clone(),
            delay_ms: 150,
        }],
    })
    .expect("export_ani");

    let ani_result = import_file(ani).expect("import ani");
    assert_eq!(ani_result.format, "ani");
    assert_eq!(ani_result.frames.len(), 1);
    assert_eq!(ani_result.frames[0].delay_ms, 150);
    assert_eq!(ani_result.frames[0].hotspot_x, 4);

    let cur = export_cur(vine_palma_cursor_lib::commands::ExportCurRequest {
        images: vec![ImageData {
            width: 8,
            height: 8,
            hotspot_x: 4,
            hotspot_y: 4,
            rgba: rgba.clone(),
        }],
    })
    .expect("export_cur");
    let cur_result = import_file(cur).expect("import cur");
    assert_eq!(cur_result.format, "cur");
    assert_eq!(cur_result.frames.len(), 1);
    assert_eq!(cur_result.frames[0].width, 8);
}