use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::stream::ByteStream;
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
                report.push(Finding::ok(
                    "DECODE_SKIPPED",
                    format!("{label} 深度解码因资源限制未执行（结构校验已完成）：{msg}"),
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

/// 基于 marker 的 JPEG 结构遍历（流式，不限文件大小）
fn verify_jpeg_structure(path: &Path, size: u64, report: &mut Report) {
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
    let mut s = ByteStream::new(&mut r);

    let mut sig = [0u8; 2];
    if !s.read_exact(&mut sig) || sig != [0xFF, 0xD8] {
        report.push(Finding::corrupt(
            "JPEG_BAD_SIG",
            "缺少 SOI 标记（FF D8），文件头已损坏",
            Some(0),
        ));
        return;
    }

    let mut found_eoi = false;

    'outer: while s.pos() < size {
        let Some(b) = s.byte() else { break };
        if b != 0xFF {
            let at = s.pos() - 1;
            report.push(Finding::corrupt(
                "JPEG_BAD_MARKER",
                format!("偏移 {at} 处缺少 0xFF 标记前缀，结构已损坏"),
                Some(at),
            ));
            return;
        }
        // 跳过 FF 填充串，取标记字节
        let (marker, marker_pos) = loop {
            let Some(c) = s.byte() else { break 'outer }; // 文件以 FF 结尾 → 未见 EOI
            if c == 0xFF {
                continue;
            }
            break (c, s.pos() - 1);
        };

        match marker {
            0x00 => {
                report.push(Finding::corrupt(
                    "JPEG_BAD_MARKER",
                    format!("偏移 {marker_pos} 处出现扫描数据外的 FF00 填充"),
                    Some(marker_pos),
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
                let len_pos = s.pos();
                let Some(seglen) = read_seglen_stream(&mut s, marker, marker_pos, report) else {
                    return;
                };
                let target = len_pos + seglen as u64;
                if target > size {
                    report.push(Finding::corrupt(
                        "JPEG_TRUNCATED",
                        format!("SOS 段超出文件范围（偏移 {marker_pos}）"),
                        Some(marker_pos),
                    ));
                    return;
                }
                s.seek_to(target);

                // 扫描熵数据：FF00 填充字节与 RSTn 直接放过；EOF → 交回外层（未见 EOI）
                let mut eoi = false;
                while let Some(x) = s.byte() {
                    if x != 0xFF {
                        continue;
                    }
                    let ff_pos = s.pos() - 1;
                    // 跳过 FF 串取后继字节
                    let mut m = None;
                    loop {
                        match s.byte() {
                            None => break,
                            Some(0xFF) => continue,
                            Some(c) => {
                                m = Some(c);
                                break;
                            }
                        }
                    }
                    match m {
                        None => break, // EOF
                        Some(0x00) | Some(0xD0..=0xD7) => continue,
                        Some(0xD9) => {
                            eoi = true;
                            break;
                        }
                        Some(_) => {
                            s.seek_to(ff_pos); // 其他标记交回外层
                            break;
                        }
                    }
                }
                if eoi {
                    found_eoi = true;
                    break 'outer;
                }
            }
            _ => {
                let len_pos = s.pos();
                let Some(seglen) = read_seglen_stream(&mut s, marker, marker_pos, report) else {
                    return;
                };
                let target = len_pos + seglen as u64;
                if target > size {
                    report.push(Finding::corrupt(
                        "JPEG_TRUNCATED",
                        format!(
                            "标记 FF{marker:02X} 段数据超出文件范围（偏移 {marker_pos}），疑似截断"
                        ),
                        Some(marker_pos),
                    ));
                    return;
                }
                s.seek_to(target);
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
    } else if s.pos() < size {
        report.push(Finding::warning(
            "JPEG_TRAILING_DATA",
            format!("EOI 之后仍有 {} 字节多余数据", size - s.pos()),
            Some(s.pos()),
        ));
    }
}

/// 流式读取标记段长度字段（2 字节大端）
fn read_seglen_stream<R: std::io::BufRead + Seek>(
    s: &mut ByteStream<R>,
    marker: u8,
    marker_pos: u64,
    report: &mut Report,
) -> Option<u16> {
    let len_pos = s.pos();
    let mut b = [0u8; 2];
    if !s.read_exact(&mut b) {
        report.push(Finding::corrupt(
            "JPEG_TRUNCATED",
            format!("标记 FF{marker:02X} 缺少长度字段"),
            Some(marker_pos),
        ));
        return None;
    }
    let seglen = u16::from_be_bytes(b);
    if seglen < 2 {
        report.push(Finding::corrupt(
            "JPEG_BAD_SEGMENT",
            format!("标记 FF{marker:02X} 的段长度 {seglen} 非法"),
            Some(len_pos),
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
fn skip_gif_sub_blocks<R: std::io::BufRead + Seek>(s: &mut ByteStream<R>, size: u64) -> bool {
    loop {
        let Some(sz) = s.byte() else { return false };
        if sz == 0 {
            return true;
        }
        let sz = u64::from(sz);
        if s.pos() + sz > size {
            return false;
        }
        s.skip(sz);
    }
}

/// GIF 结构遍历（流式，不限文件大小）
fn verify_gif_structure(path: &Path, size: u64, report: &mut Report) {
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
    let mut s = ByteStream::new(&mut r);

    let mut head = [0u8; 13];
    let n = s.read_up_to(&mut head);
    if n < 13 {
        report.push(Finding::corrupt(
            "GIF_TRUNCATED",
            format!("文件仅 {n} 字节，不足 GIF 逻辑屏幕描述符（13 字节）"),
            Some(0),
        ));
        return;
    }
    if !head.starts_with(b"GIF87a") && !head.starts_with(b"GIF89a") {
        report.push(Finding::corrupt(
            "GIF_BAD_SIG",
            "缺少 GIF 签名（GIF87a/GIF89a），文件头已损坏",
            Some(0),
        ));
        return;
    }

    let packed = head[10];
    if packed & 0x80 != 0 {
        let gct = 3u64 << ((packed & 0x07) + 1);
        if s.pos() + gct > size {
            report.push(Finding::corrupt(
                "GIF_TRUNCATED",
                format!("全局颜色表需要 {gct} 字节，超出文件范围"),
                Some(13),
            ));
            return;
        }
        s.skip(gct);
    }

    let mut trailer: Option<u64> = None;
    let mut images = 0usize;

    while s.pos() < size {
        let Some(b) = s.byte() else { break };
        match b {
            0x3B => {
                trailer = Some(s.pos() - 1);
                break;
            }
            0x21 => {
                let block_at = s.pos() - 1;
                if s.byte().is_none() {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {block_at} 处的扩展块缺少标签字节"),
                        Some(block_at),
                    ));
                    return;
                }
                if !skip_gif_sub_blocks(&mut s, size) {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {block_at} 处的扩展块数据不完整"),
                        Some(block_at),
                    ));
                    return;
                }
            }
            0x2C => {
                images += 1;
                let desc_at = s.pos() - 1;
                // 0x2C 之外还剩 9 字节（left/top/宽/高/packed）
                let mut idesc = [0u8; 9];
                if s.read_up_to(&mut idesc) < 9 {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {desc_at} 处的图像描述符不完整"),
                        Some(desc_at),
                    ));
                    return;
                }
                let ip = idesc[8];
                if ip & 0x80 != 0 {
                    let lct = 3u64 << ((ip & 0x07) + 1);
                    if s.pos() + lct > size {
                        report.push(Finding::corrupt(
                            "GIF_TRUNCATED",
                            format!("局部颜色表需要 {lct} 字节，超出文件范围"),
                            Some(s.pos()),
                        ));
                        return;
                    }
                    s.skip(lct);
                }
                let lzw_pos = s.pos();
                if s.byte().is_none() {
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        "缺少 LZW 最小码长字节",
                        Some(lzw_pos),
                    ));
                    return;
                }
                if !skip_gif_sub_blocks(&mut s, size) {
                    let fail_pos = s.pos();
                    report.push(Finding::corrupt(
                        "GIF_TRUNCATED",
                        format!("偏移 {fail_pos} 处的 LZW 数据块不完整"),
                        Some(fail_pos),
                    ));
                    return;
                }
            }
            b => {
                let at = s.pos() - 1;
                report.push(Finding::corrupt(
                    "GIF_BAD_BLOCK",
                    format!("偏移 {at} 处块标识非法（0x{b:02X}），结构已损坏"),
                    Some(at),
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
            Some(tr),
        ));
        return;
    }
    if s.pos() < size {
        report.push(Finding::warning(
            "GIF_TRAILING_DATA",
            format!("结束符之后仍有 {} 字节多余数据", size - s.pos()),
            Some(s.pos()),
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

/// BMP 结构校验：只读文件头（54 字节内），全部界检查对照文件大小，不限文件大小
fn verify_bmp_structure(path: &Path, size: u64, report: &mut Report) {
    if size < 26 {
        report.push(Finding::corrupt(
            "BMP_TRUNCATED",
            format!("文件仅 {size} 字节，不足 BMP 文件头 + DIB 头"),
            Some(0),
        ));
        return;
    }
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
    let mut head = [0u8; 54];
    let n = (&mut r).take(54).read(&mut head).unwrap_or(0);
    if n < 26 {
        report.push(Finding::corrupt(
            "BMP_TRUNCATED",
            format!("文件仅 {n} 字节，不足 BMP 文件头 + DIB 头"),
            Some(0),
        ));
        return;
    }
    let data = &head[..n];

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
    if size < 14 + u64::from(dib) {
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
        if n < 54 {
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
    if data_off < 14 + u64::from(dib) || data_off > size {
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
    } else if comp == 1 || comp == 2 {
        // RLE8/RLE4 压缩流：无法预测解压后大小，但可遍历指令流验证
        // 终止符、行宽/行数越界与数据完整性（内容级结构校验）
        let data_len = if img_size != 0 {
            u64::from(img_size)
        } else {
            size - data_off
        };
        if data_off.saturating_add(data_len) > size {
            report.push(Finding::corrupt(
                "BMP_TRUNCATED",
                format!("RLE 数据区声明 {data_len} 字节（自偏移 {data_off} 起），文件实际不足"),
                Some(data_off),
            ));
            return;
        }
        verify_bmp_rle(path, data_off, data_len, w, h.unsigned_abs(), comp, report);
    }
}

/// 读满 `n` 字节；越界/短读返回 false（`cur` 保持在失败前的已消费位置）
fn rle_read(f: &mut File, cur: &mut u64, end: u64, buf: &mut [u8]) -> bool {
    if *cur + buf.len() as u64 > end {
        return false;
    }
    if f.seek(std::io::SeekFrom::Start(*cur)).is_err() {
        return false;
    }
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..]) {
            Ok(0) | Err(_) => return false,
            Ok(k) => got += k,
        }
    }
    *cur += buf.len() as u64;
    true
}

/// RLE8/RLE4 指令流遍历。指令：`(count, value)`
/// - `count > 0`：run-length，重复像素 `count` 次
/// - `count == 0`：value=0 行结束、value=1 EOF、value=2 delta(dx,dy)、否则绝对模式
fn verify_bmp_rle(
    path: &Path,
    data_off: u64,
    data_len: u64,
    w: i64,
    h: u64,
    comp: u32,
    report: &mut Report,
) {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("打开文件失败：{e}"),
                Some(data_off),
            ));
            return;
        }
    };
    let end = data_off + data_len;
    let row_px = w.max(0) as u64;
    let mut cur = data_off;
    let mut x = 0u64;
    let mut y = 0u64;
    let mut eof = false;
    let out_of_range = |report: &mut Report, at: u64, msg: String| {
        report.push(Finding::corrupt("BMP_RLE_OUT_OF_RANGE", msg, Some(at)));
    };

    loop {
        let cmd_at = cur;
        let mut op = [0u8; 2];
        if !rle_read(&mut f, &mut cur, end, &mut op) {
            break; // 数据在此耗尽 → 收尾判 NO_EOF
        }
        let count = u64::from(op[0]);
        let value = op[1];
        if count == 0 {
            match value {
                0 => {
                    x = 0;
                    y += 1;
                    if y > h {
                        out_of_range(
                            report,
                            cmd_at,
                            format!("RLE 行数超过图像高度 {h}（指令处 y={y}）"),
                        );
                        return;
                    }
                }
                1 => {
                    eof = true;
                    break;
                }
                2 => {
                    let mut d = [0u8; 2];
                    if !rle_read(&mut f, &mut cur, end, &mut d) {
                        report.push(Finding::corrupt(
                            "BMP_RLE_TRUNCATED",
                            "delta 指令不完整（缺少位移字节）",
                            Some(cmd_at),
                        ));
                        return;
                    }
                    x += u64::from(d[0]);
                    y += u64::from(d[1]);
                    if x > row_px || y > h {
                        out_of_range(
                            report,
                            cmd_at,
                            format!("RLE delta 越界（x={x} 行宽 {row_px}，y={y} 行高 {h}）"),
                        );
                        return;
                    }
                }
                _ => {
                    // 绝对模式：value 个像素（RLE8 逐字节，RLE4 每字节 2 像素），
                    // 数据按偶数字节对齐
                    let px = u64::from(value);
                    let need = if comp == 1 { px } else { px.div_ceil(2) };
                    let padded = (need + 1) & !1;
                    if y >= h || x + px > row_px {
                        out_of_range(
                            report,
                            cmd_at,
                            format!("RLE 绝对模式越界（x={x}+{px} 行宽 {row_px}，y={y} 行高 {h}）"),
                        );
                        return;
                    }
                    if cur + padded > end {
                        report.push(Finding::corrupt(
                            "BMP_RLE_TRUNCATED",
                            format!("RLE 绝对模式数据需要 {padded} 字节，数据区不足"),
                            Some(cmd_at),
                        ));
                        return;
                    }
                    cur += padded;
                    x += px;
                }
            }
        } else {
            if y >= h || x + count > row_px {
                out_of_range(
                    report,
                    cmd_at,
                    format!("RLE run 越界（x={x}+{count} 行宽 {row_px}，y={y} 行高 {h}）"),
                );
                return;
            }
            x += count;
        }
    }

    if !eof {
        report.push(Finding::corrupt(
            "BMP_RLE_NO_EOF",
            "RLE 数据缺少结束指令（00 01），压缩流不完整或已损坏",
            Some(data_off + data_len),
        ));
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

/// WebP 结构遍历（流式逐 chunk，不限文件大小）
fn verify_webp_structure(path: &Path, size: u64, report: &mut Report) {
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
    let mut s = ByteStream::new(&mut r);

    let mut head = [0u8; 12];
    let n = s.read_up_to(&mut head);
    if n < 12 {
        report.push(Finding::corrupt(
            "WEBP_TRUNCATED",
            format!("文件仅 {n} 字节，不足 RIFF 头（12 字节）"),
            Some(0),
        ));
        return;
    }
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WEBP" {
        report.push(Finding::corrupt(
            "WEBP_BAD_SIG",
            "缺少 RIFF/WEBP 文件头，文件头已损坏",
            Some(0),
        ));
        return;
    }
    let riff = u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
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
    let end = riff + 8;

    let mut chunks = 0usize;
    while s.pos() + 8 <= end {
        let mut cb = [0u8; 8];
        if s.read_up_to(&mut cb) < 8 {
            break;
        }
        let fourcc = <[u8; 4]>::try_from(&cb[..4]).unwrap();
        let csize = u32::from_le_bytes(cb[4..8].try_into().unwrap()) as u64;
        let hdr_pos = s.pos() - 8;
        let payload = s.pos();
        let name = String::from_utf8_lossy(&fourcc).into_owned();
        if payload + csize > end {
            report.push(Finding::corrupt(
                "WEBP_TRUNCATED",
                format!("{name} 块声明 {csize} 字节，超出 RIFF 容器范围"),
                Some(hdr_pos),
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
                Some(hdr_pos),
            ));
            return;
        }
        match &fourcc {
            b"VP8X" => {
                if csize != 10 {
                    report.push(Finding::corrupt(
                        "WEBP_BAD_CHUNK",
                        format!("VP8X 扩展块长度应为 10，实际 {csize}"),
                        Some(hdr_pos),
                    ));
                    return;
                }
            }
            b"VP8L" => {
                if csize < 1 {
                    report.push(Finding::corrupt(
                        "WEBP_BAD_CHUNK",
                        "VP8L 无损流缺少签名字节 0x2F",
                        Some(payload),
                    ));
                    return;
                }
                if s.byte() != Some(0x2F) {
                    report.push(Finding::corrupt(
                        "WEBP_BAD_CHUNK",
                        "VP8L 无损流缺少签名字节 0x2F",
                        Some(payload),
                    ));
                    return;
                }
            }
            b"VP8 " if csize >= 6 => {
                let mut kb = [0u8; 6];
                if !s.read_exact(&mut kb) || [kb[3], kb[4], kb[5]] != [0x9D, 0x01, 0x2A] {
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
        s.seek_to(padded.min(end));
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

/// 基于 File+seek 的统一读取器（带边界检查）：随机访问 IFD 链，不整读文件
struct TiffReader {
    f: File,
    len: u64,
    le: bool,
    big: bool,
}

impl TiffReader {
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Option<()> {
        if off.saturating_add(buf.len() as u64) > self.len {
            return None;
        }
        use std::io::Seek;
        self.f.seek(std::io::SeekFrom::Start(off)).ok()?;
        self.f.read_exact(buf).ok()
    }

    fn u16(&mut self, off: u64) -> Option<u16> {
        let mut b = [0u8; 2];
        self.read_at(off, &mut b)?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&mut self, off: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        self.read_at(off, &mut b)?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn u64(&mut self, off: u64) -> Option<u64> {
        let mut b = [0u8; 8];
        self.read_at(off, &mut b)?;
        Some(if self.le {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        })
    }

    fn entry_size(&self) -> u64 {
        if self.big { 20 } else { 12 }
    }

    /// 值字段内联区宽度
    fn inline_size(&self) -> u64 {
        if self.big { 8 } else { 4 }
    }
}

fn tiff_type_size(ty: u16) -> Option<u64> {
    Some(match ty {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        16..=18 => 8,
        _ => return None,
    })
}

fn tiff_read_scalar(ctx: &mut TiffReader, o: u64, ty: u16) -> Option<u64> {
    match ty {
        1 | 2 | 6 | 7 => {
            let mut b = [0u8; 1];
            ctx.read_at(o, &mut b)?;
            Some(u64::from(b[0]))
        }
        3 => ctx.u16(o).map(u64::from),
        4 => ctx.u32(o).map(u64::from),
        16 => ctx.u64(o),
        _ => None,
    }
}

/// 校验一个 IFD 的条目：尺寸非零、条带/瓦片偏移落在文件内
fn tiff_scan_entries(
    ctx: &mut TiffReader,
    entry0: u64,
    n: u64,
    report: &mut Report,
) -> bool {
    let inline = ctx.inline_size();
    let len = ctx.len;
    for k in 0..n {
        let p = entry0 + k * ctx.entry_size();
        let (Some(tag), Some(ty)) = (ctx.u16(p), ctx.u16(p + 2)) else {
            report.push(Finding::corrupt(
                "TIFF_TRUNCATED",
                format!("偏移 {p} 处 IFD 条目不完整"),
                Some(p),
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
        let total = count.saturating_mul(tsz);
        let val_pos = p + if ctx.big { 12 } else { 8 };

        if total <= inline {
            match tag {
                256 | 257 => {
                    if tiff_read_scalar(ctx, val_pos, ty) == Some(0) {
                        report.push(Finding::corrupt(
                            "TIFF_BAD_DIMENSION",
                            format!("IFD 条目 {tag}（图像尺寸）为 0"),
                            Some(val_pos),
                        ));
                        return false;
                    }
                }
                273 | 324 => {
                    let slots = (count).min(inline / tsz.max(1));
                    for j in 0..slots {
                        let o = val_pos + j * tsz;
                        if let Some(v) = tiff_read_scalar(ctx, o, ty)
                            && v >= len
                        {
                            report.push(Finding::corrupt(
                                "TIFF_BAD_OFFSET",
                                format!("条目 {tag} 指向偏移 {v}，超出文件范围"),
                                Some(o),
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
                    Some(val_pos),
                ));
                return false;
            }
            if matches!(tag, 273 | 324) {
                for j in 0..count {
                    let o = arr + j * tsz;
                    if let Some(v) = tiff_read_scalar(ctx, o, ty)
                        && v >= len
                    {
                        report.push(Finding::corrupt(
                            "TIFF_BAD_OFFSET",
                            format!("条目 {tag} 第 {j} 项指向偏移 {v}，超出文件范围"),
                            Some(o),
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
    if size < 8 {
        report.push(Finding::corrupt(
            "TIFF_TRUNCATED",
            format!("文件仅 {size} 字节，不足 TIFF 头（8 字节）"),
            Some(0),
        ));
        return;
    }
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
    let mut probe = TiffReader {
        f: file,
        len: size,
        le: false,
        big: false,
    };
    let mut sig = [0u8; 8];
    if probe.read_at(0, &mut sig).is_none() {
        report.push(Finding::corrupt(
            "TIFF_TRUNCATED",
            format!("文件仅 {size} 字节，不足 TIFF 头（8 字节）"),
            Some(0),
        ));
        return;
    }
    let (le, big) = match &sig[..4] {
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
    probe.le = le;
    probe.big = big;
    let ctx = &mut probe;

    let mut off = if big {
        if size < 16 {
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

    let len = ctx.len;
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
        let n = u64::from(ctx.u16(off).unwrap_or(0));
        if n > 4096 {
            report.push(Finding::corrupt(
                "TIFF_BAD_IFD",
                format!("IFD 条目数 {n} 异常"),
                Some(off),
            ));
            return;
        }
        let entry_size = ctx.entry_size();
        let bytes = 2 + n * entry_size + if big { 8 } else { 4 };
        if off + bytes > len {
            report.push(Finding::corrupt(
                "TIFF_TRUNCATED",
                format!("IFD 需要 {bytes} 字节（偏移 {off}），超出文件范围"),
                Some(off),
            ));
            return;
        }
        if !tiff_scan_entries(ctx, off + 2, n, report) {
            return;
        }

        let next_pos = off + 2 + n * entry_size;
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

// ---------------------------------------------------------------------------
// ICO / CUR
// ---------------------------------------------------------------------------

pub struct IcoVerifier;

impl Verifier for IcoVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Ico)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Ico))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "ICO", Mode::Blind);
        verify_ico(path, &mut report);
        report
    }
}

/// ICO/CUR：ICONDIR + 图像目录条目边界遍历
fn verify_ico(path: &Path, report: &mut Report) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("打开文件失败：{e}"), None));
            return;
        }
    };
    let size = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("读取文件大小失败：{e}"), None));
            return;
        }
    };
    let mut r = BufReader::new(file);
    if size < 6 {
        report.push(Finding::corrupt(
            "ICO_TRUNCATED",
            format!("文件仅 {size} 字节，不足 ICONDIR（6 字节）"),
            Some(0),
        ));
        return;
    }
    let mut head = [0u8; 6];
    if r.read_exact(&mut head).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 ICONDIR 失败", Some(0)));
        return;
    }
    if head[0] != 0 || head[1] != 0 || head[3] != 0 {
        report.push(Finding::corrupt(
            "ICO_BAD_HEADER",
            "ICONDIR 的 reserved 字段不为 0",
            Some(0),
        ));
        return;
    }
    if head[2] != 1 && head[2] != 2 {
        report.push(Finding::corrupt(
            "ICO_BAD_HEADER",
            format!("ICONDIR type 为 {}（应为 1=ICO / 2=CUR）", head[2]),
            Some(2),
        ));
        return;
    }
    let count = u16::from_le_bytes([head[4], head[5]]);
    if count == 0 {
        report.push(Finding::corrupt(
            "ICO_NO_ENTRIES",
            "图像目录条目数为 0",
            Some(4),
        ));
        return;
    }
    let Some(dir_end) = (6u64).checked_add(u64::from(count) * 16) else {
        report.push(Finding::corrupt("ICO_DIRECTORY_TRUNCATED", "目录大小溢出", Some(4)));
        return;
    };
    if dir_end > size {
        report.push(Finding::corrupt(
            "ICO_DIRECTORY_TRUNCATED",
            format!("目录声明 {count} 条（需要 {dir_end} 字节），超出文件范围"),
            Some(4),
        ));
        return;
    }
    let mut entry = [0u8; 16];
    for i in 0..count {
        entry.iter_mut().for_each(|b| *b = 0);
        if r.read_exact(&mut entry).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取目录条目失败", Some(6)));
            return;
        }
        let at = 6u64 + u64::from(i) * 16;
        if entry[3] != 0 {
            report.push(Finding::corrupt(
                "ICO_BAD_ENTRY",
                format!("条目 {i} 的 reserved 字节不为 0"),
                Some(at + 3),
            ));
            return;
        }
        let bytes = u32::from_le_bytes(entry[8..12].try_into().unwrap());
        let offset = u64::from(u32::from_le_bytes(entry[12..16].try_into().unwrap()));
        if bytes == 0 {
            report.push(Finding::warning(
                "ICO_EMPTY_ENTRY",
                format!("条目 {i} 声明图像数据为 0 字节"),
                Some(at + 8),
            ));
            continue;
        }
        if offset.checked_add(u64::from(bytes)).is_none_or(|e| e > size) {
            report.push(Finding::corrupt(
                "ICO_ENTRY_OUT_OF_RANGE",
                format!(
                    "条目 {i} 数据区（offset={offset} size={bytes}）超出文件范围 {size}"
                ),
                Some(at + 12),
            ));
            return;
        }
        if offset < dir_end {
            report.push(Finding::warning(
                "ICO_OFFSET_OVERLAP",
                format!("条目 {i} 数据偏移 {offset} 与目录区重叠"),
                Some(at + 12),
            ));
        }
    }
}
