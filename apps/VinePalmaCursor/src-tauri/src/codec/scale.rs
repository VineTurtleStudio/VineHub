use super::bitmap::BitmapImage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InterpolationMode {
    NearestNeighbor,
    Bilinear,
}

pub fn downsample(img: &BitmapImage, target_width: u32, target_height: u32, mode: InterpolationMode) -> BitmapImage {
    if img.width == target_width && img.height == target_height {
        return img.clone();
    }

    let mut result = BitmapImage::new(target_width, target_height, 0, 0);

    match mode {
        InterpolationMode::NearestNeighbor => nearest_neighbor(img, &mut result),
        InterpolationMode::Bilinear => bilinear(img, &mut result),
    }

    let scale_x = img.width as f64 / target_width as f64;
    let scale_y = img.height as f64 / target_height as f64;
    result.hotspot_x = (img.hotspot_x as f64 / scale_x) as u32;
    result.hotspot_y = (img.hotspot_y as f64 / scale_y) as u32;

    result
}

fn nearest_neighbor(src: &BitmapImage, dst: &mut BitmapImage) {
    let scale_x = src.width as f64 / dst.width as f64;
    let scale_y = src.height as f64 / dst.height as f64;

    for y in 0..dst.height {
        for x in 0..dst.width {
            let src_x = ((x as f64 + 0.5) * scale_x) as u32;
            let src_y = ((y as f64 + 0.5) * scale_y) as u32;
            let src_x = src_x.min(src.width - 1);
            let src_y = src_y.min(src.height - 1);
            let pixel = src.get_pixel(src_x, src_y);
            dst.set_pixel(x, y, pixel);
        }
    }
}

fn bilinear(src: &BitmapImage, dst: &mut BitmapImage) {
    let scale_x = src.width as f64 / dst.width as f64;
    let scale_y = src.height as f64 / dst.height as f64;

    for y in 0..dst.height {
        for x in 0..dst.width {
            let src_x_f = (x as f64 + 0.5) * scale_x - 0.5;
            let src_y_f = (y as f64 + 0.5) * scale_y - 0.5;

            let x0 = (src_x_f.floor() as i32).max(0) as u32;
            let y0 = (src_y_f.floor() as i32).max(0) as u32;
            let x1 = (x0 + 1).min(src.width - 1);
            let y1 = (y0 + 1).min(src.height - 1);

            let fx = src_x_f - src_x_f.floor();
            let fy = src_y_f - src_y_f.floor();

            let p00 = src.get_pixel(x0, y0);
            let p10 = src.get_pixel(x1, y0);
            let p01 = src.get_pixel(x0, y1);
            let p11 = src.get_pixel(x1, y1);

            let mut pixel = [0u8; 4];
            for c in 0..4 {
                let v = p00[c] as f64 * (1.0 - fx) * (1.0 - fy)
                    + p10[c] as f64 * fx * (1.0 - fy)
                    + p01[c] as f64 * (1.0 - fx) * fy
                    + p11[c] as f64 * fx * fy;
                pixel[c] = v.round() as u8;
            }

            dst.set_pixel(x, y, pixel);
        }
    }
}

pub fn generate_sizes(source_size: u32) -> Vec<u32> {
    let mut sizes = vec![];
    let standard_sizes = [16, 24, 32, 48, 64, 96, 128, 256];

    for &s in &standard_sizes {
        if s <= source_size {
            sizes.push(s);
        }
    }

    if sizes.is_empty() {
        sizes.push(source_size);
    }

    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nearest_neighbor_downsample() {
        let mut img = BitmapImage::new(64, 64, 32, 32);
        img.fill([255, 0, 0, 255]);
        img.set_pixel(0, 0, [0, 255, 0, 255]);

        let result = downsample(&img, 32, 32, InterpolationMode::NearestNeighbor);
        assert_eq!(result.width, 32);
        assert_eq!(result.height, 32);
        assert_eq!(result.hotspot_x, 16);
        assert_eq!(result.hotspot_y, 16);
    }

    #[test]
    fn test_bilinear_downsample() {
        let mut img = BitmapImage::new(64, 64, 32, 32);
        img.fill([255, 0, 0, 255]);

        let result = downsample(&img, 16, 16, InterpolationMode::Bilinear);
        assert_eq!(result.width, 16);
        assert_eq!(result.height, 16);
    }

    #[test]
    fn test_generate_sizes() {
        let sizes = generate_sizes(96);
        assert!(sizes.contains(&16));
        assert!(sizes.contains(&32));
        assert!(sizes.contains(&48));
        assert!(sizes.contains(&96));
        assert!(!sizes.contains(&128));
    }
}
