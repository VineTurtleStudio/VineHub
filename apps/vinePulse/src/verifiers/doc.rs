use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Verifier};
use crate::verifiers::archive::{file_size, read_range};

// ---------------------------------------------------------------------------
// PDF
// ---------------------------------------------------------------------------

pub struct PdfVerifier;

impl Verifier for PdfVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Pdf)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Pdf))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "PDF", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        verify_pdf(path, size, &mut report);
        // 全面模式：逐流校验 FlateDecode 内容的可解压性
        if opts.full {
            verify_pdf_streams(path, size, &mut report);
        }
        report
    }
}

fn verify_pdf(path: &Path, size: u64, report: &mut Report) {
    // 文件头：规范允许 %PDF- 出现在前 1024 字节内
    let head_len = size.min(1024) as usize;
    let Some(head) = read_range(path, 0, head_len, report, "PDF 头") else {
        return;
    };
    let Some(hdr_pos) = head.windows(5).position(|w| w == b"%PDF-") else {
        report.push(Finding::corrupt(
            "PDF_BAD_HEADER",
            "前 1024 字节内缺少 %PDF- 文件头",
            Some(0),
        ));
        return;
    };

    // 尾部：%%EOF 结束标记与 startxref
    let tail_len = size.min(2048) as usize;
    let tail_off = size - tail_len as u64;
    let Some(tail) = read_range(path, tail_off, tail_len, report, "PDF 尾部") else {
        return;
    };
    if !tail.windows(5).any(|w| w == b"%%EOF") {
        report.push(Finding::corrupt(
            "PDF_NO_EOF",
            "尾部缺少 %%EOF 结束标记（文件可能被截断）",
            None,
        ));
        return;
    }
    let Some(sx_rel) = tail.windows(9).rposition(|w| w == b"startxref") else {
        report.push(Finding::corrupt(
            "PDF_NO_STARTXREF",
            "尾部缺少 startxref 记录",
            None,
        ));
        return;
    };
    let mut j = sx_rel + 9;
    while j < tail.len() && tail[j].is_ascii_whitespace() {
        j += 1;
    }
    let num_start = j;
    while j < tail.len() && tail[j].is_ascii_digit() {
        j += 1;
    }
    let parsed = std::str::from_utf8(&tail[num_start..j])
        .ok()
        .and_then(|s| s.parse::<u64>().ok());
    let Some(xref_off) = parsed else {
        report.push(Finding::corrupt(
            "PDF_BAD_STARTXREF",
            "startxref 之后缺少合法的交叉引用偏移",
            Some(tail_off + sx_rel as u64),
        ));
        return;
    };
    if xref_off + 4 > size {
        report.push(Finding::corrupt(
            "PDF_BAD_STARTXREF",
            format!("startxref 指向偏移 {xref_off}，超出文件末尾 {size}"),
            Some(tail_off + sx_rel as u64),
        ));
        return;
    }
    let Some(at) = read_range(path, xref_off, 8, report, "PDF xref 区") else {
        return;
    };
    // 经典 xref 表以 "xref" 开头；xref stream 则是 "N G obj" 对象
    if !at.starts_with(b"xref") && !at.first().is_some_and(u8::is_ascii_digit) {
        report.push(Finding::corrupt(
            "PDF_BAD_XREF",
            format!("startxref 指向偏移 {xref_off}，但该处既不是 xref 表也不是对象"),
            Some(xref_off),
        ));
        return;
    }
    let _ = hdr_pos;
}

// ---------------------------------------------------------------------------
// PDF 流级内容校验（全面模式）
// ---------------------------------------------------------------------------

/// 全面模式：定位带 `/Filter /FlateDecode` 的流并尝试解压，检出内容级损坏。
///
/// 防误报设计：`/Length` 与 `/Filter` 必须落在 stream 前**最后一个 `<<` 之后**
/// （即本流 dict 内），否则跳过该流——宁可漏检也不误报。
fn verify_pdf_streams(path: &Path, size: u64, report: &mut Report) {
    const SCAN_LIMIT: u64 = 256 * 1024 * 1024;
    const DECOMP_LIMIT: u64 = 512 * 1024 * 1024;
    if size > SCAN_LIMIT {
        report.push(Finding::ok(
            "PDF_STREAMS_SKIPPED",
            format!(
                "文件超过 {} MiB，流级解压校验未执行",
                SCAN_LIMIT / (1024 * 1024)
            ),
        ));
        return;
    }
    let Some(data) = read_range(path, 0, size as usize, report, "PDF 全文") else {
        return;
    };

    let mut scanned = 0u32;
    let mut checked = 0u32;
    let mut pos = 0usize;
    while pos + 6 <= data.len() {
        let Some(rel) = data[pos..].windows(6).position(|w| w == b"stream") else {
            break;
        };
        let p = pos + rel;
        pos = p + 6;
        // 跳过 "endstream" 内部的 "stream"
        if p >= 3 && &data[p - 3..p] == b"end" {
            continue;
        }
        scanned += 1;
        if scanned > 10_000 {
            report.push(Finding::warning(
                "PDF_TOO_MANY_STREAMS",
                "流数量超过 10000，剩余未校验",
                None,
            ));
            break;
        }

        // 本流 dict = stream 前最后一个 "<<"
        let win_lo = p.saturating_sub(4096);
        let Some(lt_rel) = data[win_lo..p]
            .windows(2)
            .rposition(|w| w == b"<<")
        else {
            continue;
        };
        let dict = &data[win_lo + lt_rel + 2..p];

        // /Length：直接整数或 "N G R" 间接引用（对象定义到全文找）
        let Some(len) = pdf_dict_length(dict, &data) else {
            continue;
        };
        // /Filter：仅处理直接的单值 /FlateDecode
        if pdf_dict_filter(dict) != 1 {
            continue;
        }
        // 流数据 EOL：规范要求 \r\n 或 \n（或旧式 \r）
        let mut s = p + 6;
        if s + 1 < data.len() && data[s] == b'\r' && data[s + 1] == b'\n' {
            s += 2;
        } else if s < data.len() && (data[s] == b'\n' || data[s] == b'\r') {
            s += 1;
        } else {
            continue;
        }
        let Some(e) = (s as u64).checked_add(len) else {
            continue;
        };
        if e > data.len() as u64 {
            report.push(Finding::corrupt(
                "PDF_STREAM_TRUNCATED",
                format!("第 {scanned} 个流声明 {len} 字节，超出文件范围"),
                Some(s as u64),
            ));
            return;
        }
        // zlib 解压（限制输出上限，防压缩炸弹）
        let decoder = flate2::read::ZlibDecoder::new(&data[s..e as usize]);
        let mut limited = decoder.take(DECOMP_LIMIT);
        let mut out = Vec::new();
        match limited.read_to_end(&mut out) {
            Ok(_) if out.len() as u64 >= DECOMP_LIMIT => {
                report.push(Finding::warning(
                    "PDF_STREAM_BOMB",
                    format!("第 {scanned} 个流的解压输出达 {DECOMP_LIMIT} 字节上限（疑似压缩炸弹）"),
                    Some(s as u64),
                ));
                return;
            }
            Ok(_) => checked += 1,
            Err(err) => {
                report.push(Finding::corrupt(
                    "PDF_STREAM_DECODE_FAILED",
                    format!("第 {scanned} 个 FlateDecode 流解压失败：{err}"),
                    Some(s as u64),
                ));
                return;
            }
        }
    }
    if checked > 0 {
        report.push(Finding::ok(
            "PDF_STREAMS_CHECKED",
            format!("{checked} 个 FlateDecode 流全部解压通过"),
        ));
    }
}

/// 从 dict 中取最后一个 `/Length`：直接整数，或 `N G R` 间接引用（对象定义到全文找）
fn pdf_dict_length(dict: &[u8], data: &[u8]) -> Option<u64> {
    let mut i = data_chunk_rfind(dict, b"/Length")?;
    i += 7;
    i = skip_ws(dict, i);
    let n1 = parse_dec(dict, i)?;
    let n1_end = i + n1;
    let n1_val: u64 = std::str::from_utf8(&dict[i..n1_end]).ok()?.parse().ok()?;
    let j = skip_ws(dict, n1_end);
    // 间接引用：`N G R`
    if j < dict.len() && dict[j].is_ascii_digit() {
        let n2 = parse_dec(dict, j)?;
        let g_end = j + n2;
        let k = skip_ws(dict, g_end);
        if k < dict.len() && dict[k] == b'R' {
            return resolve_length_ref(data, n1_val);
        }
        return None;
    }
    Some(n1_val)
}

/// 解析 `N G obj` 后的整数（PDF 间接对象取值）
fn resolve_length_ref(data: &[u8], obj_num: u64) -> Option<u64> {
    let needle = format!("{obj_num} ");
    let mut from = 0usize;
    while let Some(rel) = data[from..]
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())
    {
        let p = from + rel;
        from = p + needle.len();
        // 数字边界：前一字符不能是数字（避免 "15 0 obj" 被当成对象 5）
        if p > 0 && data[p - 1].is_ascii_digit() {
            continue;
        }
        let mut i = skip_ws(data, from);
        let Some(n2) = parse_dec(data, i) else {
            continue;
        };
        i = skip_ws(data, i + n2);
        if i + 3 > data.len() || &data[i..i + 3] != b"obj" {
            continue;
        }
        i = skip_ws(data, i + 3);
        let Some(n) = parse_dec(data, i) else {
            continue;
        };
        if let Ok(val) = std::str::from_utf8(&data[i..i + n]).ok()?.parse::<u64>() {
            return Some(val);
        }
    }
    None
}

fn pdf_dict_filter(dict: &[u8]) -> u8 {
    let Some(mut i) = data_chunk_rfind(dict, b"/Filter") else {
        return 0;
    };
    i = skip_ws(dict, i + 7);
    if i >= dict.len() {
        return 2;
    }
    if dict[i..].starts_with(b"/FlateDecode") {
        1
    } else {
        // 数组形式（多过滤器组合）或其它过滤器，一律跳过
        2
    }
}

/// 返回最后一次出现 `pat` 的起始下标
fn data_chunk_rfind(hay: &[u8], pat: &[u8]) -> Option<usize> {
    if hay.len() < pat.len() {
        return None;
    }
    (0..=hay.len() - pat.len())
        .rev()
        .find(|&i| &hay[i..i + pat.len()] == pat)
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b'\r' || b[i] == b'\n') {
        i += 1;
    }
    i
}

/// 解析十进制数，返回消耗的字节数
fn parse_dec(b: &[u8], i: usize) -> Option<usize> {
    let start = i;
    let mut i = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    (i > start).then_some(i - start)
}

// ---------------------------------------------------------------------------
// RTF
// ---------------------------------------------------------------------------

pub struct RtfVerifier;

impl Verifier for RtfVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Rtf)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Rtf))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "RTF", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        let head = match read_range(path, 0, size.min(64) as usize, &mut report, "RTF 头") {
            Some(h) => h,
            None => return report,
        };
        if !head.starts_with(b"{\\rtf") {
            report.push(Finding::corrupt(
                "RTF_BAD_HEADER",
                "缺少 {\\rtf 文件头",
                Some(0),
            ));
            return report;
        }
        let tail = match read_range(
            path,
            size - size.min(64),
            size.min(64) as usize,
            &mut report,
            "RTF 尾部",
        ) {
            Some(t) => t,
            None => return report,
        };
        let last = tail.iter().rev().find(|&&b| !b.is_ascii_whitespace());
        if last != Some(&b'}') {
            report.push(Finding::corrupt(
                "RTF_NO_END",
                "文件未以 } 正常结束（可能被截断）",
                None,
            ));
        }
        report
    }
}

// ---------------------------------------------------------------------------
// OOXML / ODF / EPUB 容器
// ---------------------------------------------------------------------------

pub struct DocContainerVerifier;

impl Verifier for DocContainerVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        let container = matches!(
            hint.ext.as_str(),
            "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub"
        );
        container && (hint.magic.is_none() || hint.magic == Some(MagicKind::Zip))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let label = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_uppercase())
            .unwrap_or_else(|| "DOC".to_string());
        let mut report = Report::new(path.to_path_buf(), &label, Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }

        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("打开文件失败：{e}"),
                    None,
                ));
                return report;
            }
        };
        // 打不开交给 ZIP 验证器报具体结论
        let Ok(mut zip) = zip::ZipArchive::new(BufReader::new(file)) else {
            return report;
        };

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let required: &[&str] = match ext.as_str() {
            "docx" => &["[Content_Types].xml", "word/document.xml"],
            "xlsx" => &["[Content_Types].xml", "xl/workbook.xml"],
            "pptx" => &["[Content_Types].xml", "ppt/presentation.xml"],
            "odt" | "ods" | "odp" => &["mimetype", "META-INF/manifest.xml"],
            "epub" => &["mimetype", "META-INF/container.xml"],
            _ => &[],
        };
        for name in required {
            if zip.by_name(name).is_err() {
                report.push(Finding::corrupt(
                    "DOC_MISSING_ENTRY",
                    format!("容器缺少必备条目 {name}"),
                    None,
                ));
                return report;
            }
        }

        // ODF/EPUB 的 mimetype 必须与扩展名一致
        if matches!(ext.as_str(), "odt" | "ods" | "odp" | "epub") {
            let want = if ext == "epub" {
                "application/epub+zip"
            } else {
                "application/vnd.oasis.opendocument."
            };
            if let Ok(mut entry) = zip.by_name("mimetype") {
                let mut buf = Vec::new();
                if entry.read_to_end(&mut buf).is_ok() {
                    let text = String::from_utf8_lossy(&buf);
                    let text = text.trim();
                    let ok = if ext == "epub" {
                        text == want
                    } else {
                        text.starts_with(want)
                    };
                    if !ok {
                        report.push(Finding::corrupt(
                            "DOC_BAD_MIMETYPE",
                            format!("mimetype 声明为 {text:?}，与 .{ext} 不符"),
                            None,
                        ));
                        return report;
                    }
                }
            }
        }
        report
    }
}

// ---------------------------------------------------------------------------
// 纯文本（TXT/LOG/MD）
// ---------------------------------------------------------------------------

pub struct TextDocVerifier;

impl Verifier for TextDocVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic.is_none() && matches!(hint.ext.as_str(), "txt" | "log" | "md")
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "TXT", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        let cap = 16 * 1024 * 1024;
        let Some(data) = read_range(path, 0, size.min(cap) as usize, &mut report, "文本内容") else {
            return report;
        };
        check_text_encoding(&data, &mut report);
        report
    }
}

/// 文本编码校验（TXT/CSV 共用）：
/// - 声明了 BOM 却内容不合法 → Corrupt
/// - 文本中出现 NUL → Warning
/// - 无 BOM 且非 UTF-8（如 GBK）→ 不报错，保持兼容
pub(crate) fn check_text_encoding(data: &[u8], report: &mut Report) {
    let mut utf16 = false;
    if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        if let Err(e) = std::str::from_utf8(&data[3..]) {
            report.push(Finding::corrupt(
                "TEXT_BAD_UTF8",
                format!("带 UTF-8 BOM 但内容不是合法 UTF-8：{e}"),
                Some((3 + e.valid_up_to()) as u64),
            ));
        }
    } else if data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]) {
        utf16 = true;
        let be = data[0] == 0xFE;
        let body = &data[2..];
        if !body.len().is_multiple_of(2) {
            report.push(Finding::corrupt(
                "TEXT_BAD_UTF16",
                "带 UTF-16 BOM 但内容长度为奇数",
                Some(2),
            ));
        } else {
            let units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    if be {
                        u16::from_be_bytes(*c)
                    } else {
                        u16::from_le_bytes(*c)
                    }
                })
                .collect();
            if let Err(e) = String::from_utf16(&units) {
                report.push(Finding::corrupt(
                    "TEXT_BAD_UTF16",
                    format!("带 UTF-16 BOM 但内容不是合法 UTF-16：{e}"),
                    Some(2),
                ));
            }
        }
    }
    // UTF-16 文本中的 0x00 属于正常编码，不视为二进制混入
    if !utf16
        && let Some(pos) = data.iter().position(|&b| b == 0)
    {
        report.push(Finding::warning(
            "TEXT_HAS_NUL",
            format!("文本中出现 NUL 字节（疑似二进制混入），位于 {pos}"),
            Some(pos as u64),
        ));
    }
}
