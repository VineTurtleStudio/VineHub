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
