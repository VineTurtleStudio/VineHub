use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Severity, Verifier};

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub struct PngVerifier;

impl Verifier for PngVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Png)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Png))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "PNG", Mode::Blind);
        let size = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("无法读取文件：{e}"),
                    None,
                ));
                return report;
            }
        };
        if size == 0 {
            return report;
        }

        verify_png_structure(path, size, &mut report);
        if !report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Corrupt)
        {
            deep_decode(path, opts, &mut report, "PNG_DECODE_FAILED", "PNG");
        }
        report
    }
}

/// 逐块遍历 PNG，校验签名、块长度、CRC、IHDR/IEND
fn verify_png_structure(path: &Path, size: u64, report: &mut Report) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("打开文件失败：{e}"),
                None,
            ));
            return;
        }
    };
    let mut r = BufReader::new(file);

    // 签名
    let mut sig = [0u8; 8];
    let got = read_up_to(&mut r, &mut sig);
    if got < 8 || sig != PNG_SIG {
        report.push(Finding::corrupt(
            "PNG_BAD_SIG",
            "缺少 PNG 文件签名（89 50 4E 47 0D 0A 1A 0A），文件头已损坏",
            Some(0),
        ));
        return;
    }
    let mut pos: u64 = 8;

    let mut seen_ihdr = false;
    let mut seen_iend = false;
    let mut first = true;
    let mut ihdr_data: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];

    while !seen_iend {
        let chunk_start = pos;

        let mut lenbuf = [0u8; 4];
        if !read_exact_opt(&mut r, &mut lenbuf) {
            break; // 提前 EOF → 交给下方 NO_IEND
        }
        let len = u32::from_be_bytes(lenbuf) as u64;
        pos += 4;

        let mut tbuf = [0u8; 4];
        if !read_exact_opt(&mut r, &mut tbuf) {
            report.push(Finding::corrupt(
                "PNG_TRUNCATED",
                "块类型字段被截断",
                Some(chunk_start),
            ));
            return;
        }
        pos += 4;
        let ctype = String::from_utf8_lossy(&tbuf).into_owned();

        if chunk_start + 12 + len > size {
            report.push(Finding::corrupt(
                "PNG_TRUNCATED",
                format!("{ctype} 块声明长度 {len} 字节，超出文件范围（疑似截断）"),
                Some(chunk_start),
            ));
            return;
        }

        if first {
            if tbuf != *b"IHDR" {
                report.push(Finding::corrupt(
                    "PNG_IHDR_NOT_FIRST",
                    "第一个块不是 IHDR",
                    Some(8),
                ));
            }
            first = false;
        }

        // 读数据并计算 CRC（类型 + 数据）
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&tbuf);
        let mut remaining = len;
        let mut truncated = false;
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = read_up_to(&mut r, &mut buf[..want]);
            if n == 0 {
                truncated = true;
                break;
            }
            hasher.update(&buf[..n]);
            if tbuf == *b"IHDR" && ihdr_data.len() < 13 {
                let take = (13 - ihdr_data.len()).min(n);
                ihdr_data.extend_from_slice(&buf[..take]);
            }
            pos += n as u64;
            remaining -= n as u64;
        }
        if truncated {
            report.push(Finding::corrupt(
                "PNG_TRUNCATED",
                format!("{ctype} 块数据不完整（文件被截断）"),
                Some(chunk_start),
            ));
            return;
        }

        let mut crcbuf = [0u8; 4];
        if !read_exact_opt(&mut r, &mut crcbuf) {
            report.push(Finding::corrupt(
                "PNG_TRUNCATED",
                format!("{ctype} 块缺少 CRC 字段"),
                Some(pos),
            ));
            return;
        }
        let crc_pos = pos;
        pos += 4;
        let stored = u32::from_be_bytes(crcbuf);
        let computed = hasher.finalize();
        if stored != computed {
            report.push(Finding::corrupt(
                "PNG_BAD_CRC",
                format!("{ctype} 块 CRC 不匹配：期望 {computed:08x}，实际 {stored:08x}"),
                Some(crc_pos),
            ));
        }

        match &tbuf {
            b"IHDR" => {
                seen_ihdr = true;
                if len != 13 {
                    report.push(Finding::corrupt(
                        "PNG_BAD_IHDR",
                        format!("IHDR 块长度应为 13，实际 {len}"),
                        Some(chunk_start),
                    ));
                } else if ihdr_data.len() == 13 {
                    let w = u32::from_be_bytes(ihdr_data[0..4].try_into().unwrap());
                    let h = u32::from_be_bytes(ihdr_data[4..8].try_into().unwrap());
                    if w == 0 || h == 0 {
                        report.push(Finding::corrupt(
                            "PNG_BAD_DIMENSIONS",
                            format!("图像尺寸非法：{w} x {h}"),
                            Some(chunk_start + 8),
                        ));
                    }
                }
            }
            b"IEND" => seen_iend = true,
            _ => {}
        }
    }

    if !seen_ihdr {
        report.push(Finding::corrupt(
            "PNG_NO_IHDR",
            "缺少 IHDR 起始块，文件头已损坏",
            Some(0),
        ));
    }
    if !seen_iend {
        report.push(Finding::corrupt(
            "PNG_NO_IEND",
            "缺少 IEND 结束块，文件可能被截断",
            Some(size),
        ));
    }
    if seen_iend && pos < size {
        report.push(Finding::warning(
            "PNG_TRAILING_DATA",
            format!("IEND 之后仍有 {} 字节多余数据", size - pos),
            Some(pos),
        ));
    }
}

/// 深度解码（依赖 image crate），失败则产出结论
pub fn deep_decode(path: &Path, opts: &Opts, report: &mut Report, code: &'static str, label: &str) {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    if !opts.should_deep(size) {
        return;
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("读取文件失败：{e}"),
                None,
            ));
            return;
        }
    };

    use image::ImageReader;
    let mut reader = match ImageReader::new(std::io::Cursor::new(&data)).with_guessed_format() {
        Ok(r) => r,
        Err(e) => {
            report.push(Finding::corrupt(
                code,
                format!("{label} 深度解码失败：{e}"),
                None,
            ));
            return;
        }
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(65535);
    limits.max_image_height = Some(65535);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);

    match reader.decode() {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            let low = msg.to_ascii_lowercase();
            if low.contains("memory") || low.contains("limit") || low.contains("too large") {
                report.push(Finding::unknown(
                    "DECODE_SKIPPED",
                    format!("{label} 深度解码因资源限制跳过：{msg}"),
                    None,
                ));
            } else {
                report.push(Finding::corrupt(
                    code,
                    format!("{label} 深度解码失败：{msg}"),
                    None,
                ));
            }
        }
    }
}

/// 尽量读满，返回实际读到的字节数
fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> usize {
    let mut total = 0;
    while total < buf.len() {
        match r.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break,
        }
    }
    total
}

/// 读满返回 true，提前 EOF 返回 false
fn read_exact_opt<R: Read>(r: &mut R, buf: &mut [u8]) -> bool {
    read_up_to(r, buf) == buf.len()
}

pub struct JpegVerifier;

impl Verifier for JpegVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Jpeg)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Jpeg))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "JPEG", Mode::Blind);
        let size = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("无法读取文件：{e}"),
                    None,
                ));
                return report;
            }
        };
        if size == 0 {
            return report;
        }

        verify_jpeg_structure(path, size, &mut report);

        let has_corrupt = report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Corrupt);
        if !has_corrupt {
            deep_decode(path, opts, &mut report, "JPEG_DECODE_FAILED", "JPEG");
        }
        report
    }
}

/// 基于 marker 的 JPEG 结构遍历
fn verify_jpeg_structure(path: &Path, size: u64, report: &mut Report) {
    if size > 512 * 1024 * 1024 {
        report.push(Finding::unknown(
            "JPEG_TOO_LARGE",
            "文件超过 512 MiB，跳过结构遍历",
            None,
        ));
        return;
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("读取文件失败：{e}"),
                None,
            ));
            return;
        }
    };

    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        report.push(Finding::corrupt(
            "JPEG_BAD_SIG",
            "缺少 SOI 标记（FF D8），文件头已损坏",
            Some(0),
        ));
        return;
    }

    let mut i = 2usize;
    let mut found_eoi = false;

    while i < data.len() {
        if data[i] != 0xFF {
            report.push(Finding::corrupt(
                "JPEG_BAD_MARKER",
                format!("偏移 {i} 处缺少 0xFF 标记前缀，结构已损坏"),
                Some(i as u64),
            ));
            return;
        }
        while i < data.len() && data[i] == 0xFF {
            i += 1;
        }
        if i >= data.len() {
            break; // 文件以 FF 结尾 → 未见 EOI
        }
        let marker = data[i];
        let marker_pos = i;
        i += 1;

        match marker {
            0x00 => {
                report.push(Finding::corrupt(
                    "JPEG_BAD_MARKER",
                    format!("偏移 {marker_pos} 处出现扫描数据外的 FF00 填充"),
                    Some(marker_pos as u64),
                ));
                return;
            }
            0xD9 => {
                found_eoi = true;
                break;
            }
            // SOI / TEM / RSTn 为独立标记，无长度字段
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xDA => {
                // SOS：段头之后是熵编码数据
                let seglen = match read_seglen(&data, i, marker_pos, report) {
                    Some(l) => l,
                    None => return,
                };
                if i + seglen as usize > data.len() {
                    report.push(Finding::corrupt(
                        "JPEG_TRUNCATED",
                        format!("SOS 段超出文件范围（偏移 {marker_pos}）"),
                        Some(marker_pos as u64),
                    ));
                    return;
                }
                i += seglen as usize;

                // 扫描熵数据：FF00 填充字节与 RSTn 直接放过
                while i < data.len() {
                    if data[i] != 0xFF {
                        i += 1;
                        continue;
                    }
                    let ff_pos = i;
                    let mut j = i + 1;
                    while j < data.len() && data[j] == 0xFF {
                        j += 1;
                    }
                    if j >= data.len() {
                        i = data.len();
                        break;
                    }
                    let m = data[j];
                    if m == 0x00 || (0xD0..=0xD7).contains(&m) {
                        i = j + 1;
                        continue;
                    }
                    if m == 0xD9 {
                        i = j + 1;
                        found_eoi = true;
                        break;
                    }
                    i = ff_pos; // 其他标记交回外层
                    break;
                }
                if found_eoi {
                    break;
                }
            }
            _ => {
                let seglen = match read_seglen(&data, i, marker_pos, report) {
                    Some(l) => l,
                    None => return,
                };
                if i + seglen as usize > data.len() {
                    report.push(Finding::corrupt(
                        "JPEG_TRUNCATED",
                        format!(
                            "标记 FF{marker:02X} 段数据超出文件范围（偏移 {marker_pos}），疑似截断"
                        ),
                        Some(marker_pos as u64),
                    ));
                    return;
                }
                i += seglen as usize;
            }
        }
    }

    if !found_eoi {
        let already = report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Corrupt);
        if !already {
            report.push(Finding::corrupt(
                "JPEG_NO_EOI",
                "缺少 EOI 结束标记（FF D9），文件可能被截断",
                Some(size),
            ));
        }
    } else if i < data.len() {
        report.push(Finding::warning(
            "JPEG_TRAILING_DATA",
            format!("EOI 之后仍有 {} 字节多余数据", data.len() - i),
            Some(i as u64),
        ));
    }
}

fn read_seglen(data: &[u8], i: usize, marker_pos: usize, report: &mut Report) -> Option<u16> {
    if i + 2 > data.len() {
        report.push(Finding::corrupt(
            "JPEG_TRUNCATED",
            format!("标记 FF{:02X} 缺少长度字段", data[marker_pos]),
            Some(marker_pos as u64),
        ));
        return None;
    }
    let seglen = u16::from_be_bytes([data[i], data[i + 1]]);
    if seglen < 2 {
        report.push(Finding::corrupt(
            "JPEG_BAD_SEGMENT",
            format!("标记 FF{:02X} 的段长度 {seglen} 非法", data[marker_pos]),
            Some(i as u64),
        ));
        return None;
    }
    Some(seglen)
}

// ---------------------------------------------------------------------------
// GIF / BMP / WebP / TIFF
// ---------------------------------------------------------------------------

/// 结构遍历 +（无损坏时）深度解码的通用流程
fn verify_image(
    path: &Path,
    opts: &Opts,
    type_name: &'static str,
    structure: fn(&Path, u64, &mut Report),
    decode_code: &'static str,
) -> Report {
    let mut report = Report::new(path.to_path_buf(), type_name, Mode::Blind);
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("无法读取文件：{e}"),
                None,
            ));
            return report;
        }
    };
    if size == 0 {
        return report;
    }

    structure(path, size, &mut report);
    let has_corrupt = report
        .findings
        .iter()
        .any(|f| f.severity == Severity::Corrupt);
    if !has_corrupt {
        deep_decode(path, opts, &mut report, decode_code, type_name);
    }
    report
}

/// 一次性读入文件；过大或读取失败时记结论并返回 None
fn read_whole(path: &Path, size: u64, report: &mut Report, label: &str) -> Option<Vec<u8>> {
    if size > 512 * 1024 * 1024 {
        report.push(Finding::unknown(
            "IMAGE_TOO_LARGE",
            format!("{label} 文件超过 512 MiB，跳过结构遍历"),
            None,
        ));
        return None;
    }
    match std::fs::read(path) {
        Ok(d) => Some(d),
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("读取文件失败：{e}"),
                None,
            ));
            None
        }
    }
}

// --- GIF ---

pub struct GifVerifier;

impl Verifier for GifVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Gif)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Gif))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        verify_image(
            path,
            opts,
            "GIF",
            verify_gif_structure,
            "GIF_DECODE_FAILED",
        )
    }
}

/// 跳过 GIF 子块链（长度字节 + 数据，直到 0 结束）；数据不足返回 false
fn skip_gif_sub_blocks(data: &[u8], i: &mut usize) -> bool {
    loop {
        if *i >= data.len() {
            return false;
        }
        let sz = data[*i] as usize;
        *i += 1;
        if sz == 0 {
            return true;
        }
        if *i + sz > data.len() {
            return false;
        }
        *i += sz;
    }
}

fn verify_gif_structure(path: &Path, size: u64, report: &mut Report) {
    let Some(data) = read_whole(path, size, report, "GIF") else {
        return;
    };
    if data.len() < 13 {
        report.push(Finding::corrupt(
            "GIF_TRUNCATED",
            format!("文件仅 {} 字节，不足 GIF 逻辑屏幕描述符（13 字节）", data.len()),
            Some(0),
        ));
        return;
    }
    if !data.starts_with(b"GIF87a") && !data.starts_with(b"GIF89a") {
        report.push(Finding::corrupt(
            "GIF_BAD_SIG",
            "缺少 GIF 签名（GIF87a/GIF89a），文件头已损坏",
            Some(0),
        ));
        return;
    }

    let packed = data[10];
    let mut i = 13usize;
    if packed & 0x80 != 0 {
        let gct = 3usize * (1usize << ((packed & 0x07) + 1));
        if i + gct > data.len() {
            report.push(Finding::corrupt(
                "GIF_TRUNCATED",
                format!("全局颜色表需要 {gct} 字节，超出文件范围"),
                Some(13),
            ));
            return;
        }
        i += gct;
    }

    let mut trailer: Option<usize> = None;
    let mut images = 0usize;

    while i < data.len() {
        match data[i] {
            0x3B => {
                trailer = Some(i);
                i += 1;
                break;
            }
            0x21 => {
                let block_at = i;
                i += 1; // 0x21
                if i >= data.len() {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {block_at} 处的扩展块缺少标签字节"),
                        Some(block_at as u64),
                    ));
                    return;
                }
                i += 1; // 扩展标签
                if !skip_gif_sub_blocks(&data, &mut i) {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {block_at} 处的扩展块数据不完整"),
                        Some(block_at as u64),
                    ));
                    return;
                }
            }
            0x2C => {
                images += 1;
                if i + 10 > data.len() {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {i} 处的图像描述符不完整"),
                        Some(i as u64),
                    ));
                    return;
                }
                let ip = data[i + 9];
                i += 10;
                if ip & 0x80 != 0 {
                    let lct = 3usize * (1usize << ((ip & 0x07) + 1));
                    if i + lct > data.len() {
                        report.push(Finding::corrupt(
                            "GIF_TRUNCATED",
                            format!("局部颜色表需要 {lct} 字节，超出文件范围"),
                            Some(i as u64),
                        ));
                        return;
                    }
                    i += lct;
                }
                if i >= data.len() {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        "缺少 LZW 最小码长字节",
                        Some(i as u64),
                    ));
                    return;
                }
                i += 1; // LZW 最小码长
                if !skip_gif_sub_blocks(&data, &mut i) {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {i} 处的 LZW 数据块不完整"),
                        Some(i as u64),
                    ));
                    return;
                }
            }
            b => {
                report.push(Finding::corrupt(
                    "GIF_BAD_BLOCK",
                    format!("偏移 {i} 处块标识非法（0x{b:02X}），结构已损坏"),
                    Some(i as u64),
                ));
                return;
            }
        }
    }

    let Some(tr) = trailer else {
        report.push(Finding::corrupt(
            "GIF_NO_TRAILER",
            "缺少 GIF 结束符（0x3B），文件可能被截断",
            Some(size),
        ));
        return;
    };
    if images == 0 {
        report.push(Finding::corrupt(
            "GIF_NO_IMAGE",
            "GIF 中没有任何图像描述符，文件内容已损坏",
            Some(tr as u64),
        ));
        return;
    }
    if i < data.len() {
        report.push(Finding::warning(
            "GIF_TRAILING_DATA",
            format!("结束符之后仍有 {} 字节多余数据", data.len() - i),
            Some(i as u64),
        ));
    }
}

// --- BMP ---

pub struct BmpVerifier;

impl Verifier for BmpVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Bmp)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Bmp))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        verify_image(
            path,
            opts,
            "BMP",
            verify_bmp_structure,
            "BMP_DECODE_FAILED",
        )
    }
}

fn verify_bmp_structure(path: &Path, size: u64, report: &mut Report) {
    let Some(data) = read_whole(path, size, report, "BMP") else {
        return;
    };
    if data.len() < 26 {
        report.push(Finding::corrupt(
            "BMP_TRUNCATED",
            format!("文件仅 {} 字节，不足 BMP 文件头 + DIB 头", data.len()),
            Some(0),
        ));
        return;
    }
    if &data[0..2] != b"BM" {
        report.push(Finding::corrupt(
            "BMP_BAD_SIG",
            "缺少 BM 签名，文件头已损坏",
            Some(0),
        ));
        return;
    }
    let declared = u32::from_le_bytes(data[2..6].try_into().unwrap()) as u64;
    let data_off = u32::from_le_bytes(data[10..14].try_into().unwrap()) as u64;
    if declared > size {
        report.push(Finding::corrupt(
            "BMP_TRUNCATED",
            format!("文件头声明 {declared} 字节，实际仅 {size} 字节（疑似截断）"),
            Some(2),
        ));
        return;
    }
    if declared > 0 && declared < size {
        report.push(Finding::warning(
            "BMP_TRAILING",
            format!("文件头声明 {declared} 字节，实际 {size} 字节，尾部多出 {} 字节", size - declared),
            Some(2),
        ));
    }

    let dib = u32::from_le_bytes(data[14..18].try_into().unwrap());
    if dib == 12 || (40..=124).contains(&dib) {
        // 合法
    } else if dib < 40 {
        report.push(Finding::corrupt(
            "BMP_BAD_DIB",
            format!("DIB 头长度 {dib} 非法"),
            Some(14),
        ));
        return;
    } else {
        report.push(Finding::warning(
            "BMP_BAD_DIB",
            format!("非典型 DIB 头长度 {dib}"),
            Some(14),
        ));
    }
    if (data.len() as u64) < 14 + dib as u64 {
        report.push(Finding::corrupt(
            "BMP_TRUNCATED",
            format!("DIB 头声明 {dib} 字节，文件不足以容纳"),
            Some(14),
        ));
        return;
    }

    let (w, h, planes, bpp, comp, img_size) = if dib == 12 {
        (
            i64::from(u16::from_le_bytes(data[18..20].try_into().unwrap())),
            i64::from(u16::from_le_bytes(data[20..22].try_into().unwrap())),
            u16::from_le_bytes(data[22..24].try_into().unwrap()),
            u16::from_le_bytes(data[24..26].try_into().unwrap()),
            0u32,
            0u32,
        )
    } else {
        if data.len() < 54 {
            report.push(Finding::corrupt(
                "BMP_TRUNCATED",
                "BITMAPINFOHEADER 头不完整",
                Some(14),
            ));
            return;
        }
        (
            i64::from(i32::from_le_bytes(data[18..22].try_into().unwrap())),
            i64::from(i32::from_le_bytes(data[22..26].try_into().unwrap())),
            u16::from_le_bytes(data[26..28].try_into().unwrap()),
            u16::from_le_bytes(data[28..30].try_into().unwrap()),
            u32::from_le_bytes(data[30..34].try_into().unwrap()),
            u32::from_le_bytes(data[34..38].try_into().unwrap()),
        )
    };

    if w <= 0 || h == 0 {
        report.push(Finding::corrupt(
            "BMP_BAD_DIMENSIONS",
            format!("图像尺寸非法：{w} x {h}"),
            Some(18),
        ));
        return;
    }
    if planes != 1 {
        report.push(Finding::warning(
            "BMP_BAD_PLANES",
            format!("平面数应为 1，实际 {planes}"),
            Some(26),
        ));
    }
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        report.push(Finding::corrupt(
            "BMP_BAD_BPP",
            format!("位深 {bpp} 非法"),
            Some(28),
        ));
        return;
    }
    if comp > 6 {
        report.push(Finding::corrupt(
            "BMP_BAD_COMPRESSION",
            format!("压缩方式 {comp} 属保留值"),
            Some(30),
        ));
        return;
    }
    if data_off < 14 + dib as u64 || data_off > size {
        report.push(Finding::corrupt(
            "BMP_BAD_DATA_OFFSET",
            format!("像素数据起始偏移 {data_off} 非法（DIB 头结束于 {}）", 14 + dib),
            Some(10),
        ));
        return;
    }
    if comp == 0 || comp == 3 {
        let row = ((w * i64::from(bpp) + 31) / 32) * 4;
        let expected = row * h.unsigned_abs() as i64;
        if data_off as i64 + expected > size as i64 {
            report.push(Finding::corrupt(
                "BMP_TRUNCATED",
                format!("像素数据需要 {expected} 字节（自偏移 {data_off} 起），文件实际不足"),
                Some(data_off),
            ));
        } else if comp == 0 && img_size != 0 && i64::from(img_size) != expected {
            report.push(Finding::warning(
                "BMP_BAD_IMAGE_SIZE",
                format!("图像数据区声明 {img_size} 字节，按尺寸计算应为 {expected}"),
                Some(34),
            ));
        }
    }
}

// --- WebP ---

pub struct WebpVerifier;

impl Verifier for WebpVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Webp)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Webp))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        verify_image(
            path,
            opts,
            "WebP",
            verify_webp_structure,
            "WEBP_DECODE_FAILED",
        )
    }
}

fn verify_webp_structure(path: &Path, size: u64, report: &mut Report) {
    let Some(data) = read_whole(path, size, report, "WebP") else {
        return;
    };
    if data.len() < 12 {
        report.push(Finding::corrupt(
            "WEBP_TRUNCATED",
            format!("文件仅 {} 字节，不足 RIFF 头（12 字节）", data.len()),
            Some(0),
        ));
        return;
    }
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        report.push(Finding::corrupt(
            "WEBP_BAD_SIG",
            "缺少 RIFF/WEBP 文件头，文件头已损坏",
            Some(0),
        ));
        return;
    }
    let riff = u32::from_le_bytes(data[4..8].try_into().unwrap()) as u64;
    if riff + 8 > size {
        report.push(Finding::corrupt(
            "WEBP_TRUNCATED",
            format!("RIFF 声明 {riff} 字节，实际文件不足（疑似截断）"),
            Some(4),
        ));
        return;
    }
    if riff + 8 < size {
        report.push(Finding::warning(
            "WEBP_TRAILING",
            format!("RIFF 声明 {} 字节，实际 {size} 字节，尾部多出 {} 字节", riff + 8, size - riff - 8),
            Some(4),
        ));
    }
    let end = (riff + 8) as usize;

    let mut i = 12usize;
    let mut chunks = 0usize;
    while i + 8 <= end {
        let Ok(fourcc) = <[u8; 4]>::try_from(&data[i..i + 4]) else {
            break;
        };
        let csize = u32::from_le_bytes(data[i + 4..i + 8].try_into().unwrap()) as u64;
        let payload = i as u64 + 8;
        let name = String::from_utf8_lossy(&fourcc).into_owned();
        if payload + csize > end as u64 {
            report.push(Finding::corrupt(
                "WEBP_TRUNCATED",
                format!("{name} 块声明 {csize} 字节，超出 RIFF 容器范围"),
                Some(i as u64),
            ));
            return;
        }
        if chunks == 0
            && fourcc != *b"VP8 "
            && fourcc != *b"VP8L"
            && fourcc != *b"VP8X"
        {
            report.push(Finding::corrupt(
                "WEBP_BAD_CHUNK",
                format!("首个数据块是 {name}，应为 VP8 / VP8L / VP8X"),
                Some(i as u64),
            ));
            return;
        }
        match &fourcc {
            b"VP8X" => {
                if csize != 10 {
                    report.push(Finding::corrupt(
                        "WEBP_BAD_CHUNK",
                        format!("VP8X 扩展块长度应为 10，实际 {csize}"),
                        Some(i as u64),
                    ));
                    return;
                }
            }
            b"VP8L" => {
                let p = payload as usize;
                if csize < 1 || data[p] != 0x2F {
                    report.push(Finding::corrupt(
                        "WEBP_BAD_CHUNK",
                        "VP8L 无损流缺少签名字节 0x2F",
                        Some(payload),
                    ));
                    return;
                }
            }
            b"VP8 " => {
                let p = payload as usize;
                if csize >= 6 && [data[p + 3], data[p + 4], data[p + 5]] != [0x9D, 0x01, 0x2A] {
                    report.push(Finding::corrupt(
                        "WEBP_BAD_CHUNK",
                        "VP8 关键帧缺少起始码 9D 01 2A",
                        Some(payload + 3),
                    ));
                    return;
                }
            }
            _ => {}
        }
        chunks += 1;
        let padded = payload + ((csize + 1) & !1);
        i = (padded.min(end as u64)) as usize;
    }

    if chunks == 0 {
        report.push(Finding::corrupt(
            "WEBP_NO_FRAME",
            "RIFF/WEBP 容器内没有任何图像数据块",
            Some(12),
        ));
    }
}

// --- TIFF ---

pub struct TiffVerifier;

impl Verifier for TiffVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Tif)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Tif))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        verify_image(
            path,
            opts,
            "TIFF",
            verify_tiff_structure,
            "TIFF_DECODE_FAILED",
        )
    }
}

/// 统一的大小端读取器（带边界检查）
struct TiffCtx<'a> {
    data: &'a [u8],
    le: bool,
    big: bool,
}

impl TiffCtx<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let b = self.data.get(o..o + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32(&self, o: usize) -> Option<u32> {
        let b = self.data.get(o..o + 4)?;
        let a: [u8; 4] = b.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }

    fn u64(&self, o: usize) -> Option<u64> {
        let b = self.data.get(o..o + 8)?;
        let a: [u8; 8] = b.try_into().ok()?;
        Some(if self.le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) })
    }

    fn entry_size(&self) -> usize {
        if self.big { 20 } else { 12 }
    }

    /// 值字段内联区宽度
    fn inline_size(&self) -> usize {
        if self.big { 8 } else { 4 }
    }
}

fn tiff_type_size(ty: u16) -> Option<usize> {
    Some(match ty {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        16..=18 => 8,
        _ => return None,
    })
}

fn tiff_read_scalar(ctx: &TiffCtx, o: usize, ty: u16) -> Option<u64> {
    match ty {
        1 | 2 | 6 | 7 => ctx.data.get(o).map(|&b| u64::from(b)),
        3 => ctx.u16(o).map(u64::from),
        4 => ctx.u32(o).map(u64::from),
        16 => ctx.u64(o),
        _ => None,
    }
}

/// 校验一个 IFD 的条目：尺寸非零、条带/瓦片偏移落在文件内
fn tiff_scan_entries(
    ctx: &TiffCtx,
    entry0: usize,
    n: usize,
    report: &mut Report,
) -> bool {
    let inline = ctx.inline_size();
    let len = ctx.data.len() as u64;
    for k in 0..n {
        let p = entry0 + k * ctx.entry_size();
        let (Some(tag), Some(ty)) = (ctx.u16(p), ctx.u16(p + 2)) else {
            report.push(Finding::corrupt(
                "TIFF_TRUNCATED",
                format!("偏移 {p} 处 IFD 条目不完整"),
                Some(p as u64),
            ));
            return false;
        };
        let count = if ctx.big {
            ctx.u64(p + 4).unwrap_or(0)
        } else {
            u64::from(ctx.u32(p + 4).unwrap_or(0))
        };
        if count == 0 || count > 1_000_000 {
            continue;
        }
        let Some(tsz) = tiff_type_size(ty) else { continue };
        let total = count.saturating_mul(tsz as u64);
        let val_pos = p + if ctx.big { 12 } else { 8 };

        if total <= inline as u64 {
            match tag {
                256 | 257 => {
                    if tiff_read_scalar(ctx, val_pos, ty) == Some(0) {
                        report.push(Finding::corrupt(
                            "TIFF_BAD_DIMENSION",
                            format!("IFD 条目 {tag}（图像尺寸）为 0"),
                            Some(val_pos as u64),
                        ));
                        return false;
                    }
                }
                273 | 324 => {
                    let slots = (count as usize).min(inline / tsz.max(1));
                    for j in 0..slots {
                        let o = val_pos + j * tsz;
                        if let Some(v) = tiff_read_scalar(ctx, o, ty)
                            && v >= len
                        {
                            report.push(Finding::corrupt(
                                "TIFF_BAD_OFFSET",
                                format!("条目 {tag} 指向偏移 {v}，超出文件范围"),
                                Some(o as u64),
                            ));
                            return false;
                        }
                    }
                }
                _ => {}
            }
        } else {
            let arr = if ctx.big {
                ctx.u64(val_pos).unwrap_or(0)
            } else {
                u64::from(ctx.u32(val_pos).unwrap_or(0))
            };
            if arr.saturating_add(total) > len {
                report.push(Finding::corrupt(
                    "TIFF_BAD_OFFSET",
                    format!("条目 {tag} 的值区（偏移 {arr}，{total} 字节）超出文件范围"),
                    Some(val_pos as u64),
                ));
                return false;
            }
            if matches!(tag, 273 | 324) {
                for j in 0..count {
                    let o = arr as usize + (j as usize) * tsz;
                    if let Some(v) = tiff_read_scalar(ctx, o, ty)
                        && v >= len
                    {
                        report.push(Finding::corrupt(
                            "TIFF_BAD_OFFSET",
                            format!("条目 {tag} 第 {j} 项指向偏移 {v}，超出文件范围"),
                            Some(o as u64),
                        ));
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn verify_tiff_structure(path: &Path, size: u64, report: &mut Report) {
    let Some(data) = read_whole(path, size, report, "TIFF") else {
        return;
    };
    if data.len() < 8 {
        report.push(Finding::corrupt(
            "TIFF_TRUNCATED",
            format!("文件仅 {} 字节，不足 TIFF 头（8 字节）", data.len()),
            Some(0),
        ));
        return;
    }
    let (le, big) = match &data[0..4] {
        b"II\x2a\x00" => (true, false),
        b"MM\x00\x2a" => (false, false),
        b"II\x2b\x00" => (true, true),
        b"MM\x00\x2b" => (false, true),
        _ => {
            report.push(Finding::corrupt(
                "TIFF_BAD_SIG",
                "缺少 TIFF 签名（II/MM + 版本号），文件头已损坏",
                Some(0),
            ));
            return;
        }
    };
    let ctx = TiffCtx {
        data: &data,
        le,
        big,
    };

    let mut off = if big {
        if data.len() < 16 {
            report.push(Finding::corrupt(
                "TIFF_TRUNCATED",
                "BigTIFF 头（16 字节）不完整",
                Some(0),
            ));
            return;
        }
        if ctx.u16(4) != Some(8) {
            report.push(Finding::corrupt(
                "TIFF_BAD_HEADER",
                "BigTIFF 的偏移字段宽度必须为 8",
                Some(4),
            ));
            return;
        }
        ctx.u64(8).unwrap_or(0)
    } else {
        u64::from(ctx.u32(4).unwrap_or(0))
    };

    if off == 0 {
        report.push(Finding::corrupt(
            "TIFF_NO_IFD",
            "首 IFD 偏移为 0，文件头已损坏",
            Some(4),
        ));
        return;
    }

    let len = data.len() as u64;
    let mut hops = 0usize;
    loop {
        if hops > 64 {
            report.push(Finding::warning(
                "TIFF_IFD_CHAIN",
                "IFD 链超过 64 个节点，已停止遍历",
                Some(off),
            ));
            break;
        }
        if off + 2 > len {
            report.push(Finding::corrupt(
                "TIFF_TRUNCATED",
                format!("IFD 偏移 {off} 超出文件范围"),
                Some(off),
            ));
            return;
        }
        let n = ctx.u16(off as usize).unwrap_or(0) as usize;
        if n > 4096 {
            report.push(Finding::corrupt(
                "TIFF_BAD_IFD",
                format!("IFD 条目数 {n} 异常"),
                Some(off),
            ));
            return;
        }
        let entry_size = ctx.entry_size() as u64;
        let bytes = 2u64 + n as u64 * entry_size + if big { 8 } else { 4 };
        if off + bytes > len {
            report.push(Finding::corrupt(
                "TIFF_TRUNCATED",
                format!("IFD 需要 {bytes} 字节（偏移 {off}），超出文件范围"),
                Some(off),
            ));
            return;
        }
        if !tiff_scan_entries(&ctx, (off + 2) as usize, n, report) {
            return;
        }

        let next_pos = (off + 2 + n as u64 * entry_size) as usize;
        let next = if big {
            ctx.u64(next_pos).unwrap_or(0)
        } else {
            u64::from(ctx.u32(next_pos).unwrap_or(0))
        };
        if next == 0 {
            break;
        }
        if next == off {
            report.push(Finding::corrupt(
                "TIFF_BAD_IFD",
                format!("IFD 链在偏移 {off} 处自指，结构已损坏"),
                Some(off),
            ));
            return;
        }
        off = next;
        hops += 1;
    }
}
