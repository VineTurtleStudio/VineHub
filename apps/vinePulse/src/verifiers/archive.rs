use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Verifier};

const EOCD_SIG: [u8; 4] = [0x50, 0x4B, 0x05, 0x06];
/// EOCD 最小长度 22 字节 + 最大注释 65535
const EOCD_SEARCH: usize = 22 + 65535;

pub struct ZipVerifier;

impl Verifier for ZipVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Zip)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Zip))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "ZIP", Mode::Blind);
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

        let eocd = verify_eocd(path, size, &mut report);
        verify_archive(path, size, eocd, opts, &mut report);
        report
    }
}

/// 手工定位中央目录结束记录（EOCD），检查截断/尾部多余数据
/// 返回 (EOCD 偏移, 条目数)
fn verify_eocd(path: &Path, size: u64, report: &mut Report) -> Option<(u64, u16)> {
    let mut f = File::open(path).ok()?;
    let tail_len = size.min(EOCD_SEARCH as u64) as usize;
    let mut tail = vec![0u8; tail_len];
    use std::io::Seek;
    f.seek(std::io::SeekFrom::Start(size - tail_len as u64))
        .ok()?;
    let mut read = 0;
    while read < tail_len {
        match f.read(&mut tail[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => break,
        }
    }
    tail.truncate(read);

    // 从后向前找签名
    let mut found: Option<usize> = None;
    if tail.len() >= 4 {
        let mut i = tail.len() - 4;
        loop {
            if tail[i..i + 4] == EOCD_SIG {
                found = Some(i);
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
    }

    let Some(rel) = found else {
        report.push(Finding::corrupt(
            "ZIP_NO_EOCD",
            "未找到中央目录结束记录（PK\\x05\\x06），文件可能被截断",
            Some(size.saturating_sub(22)),
        ));
        return None;
    };
    let abs = size - tail_len as u64 + rel as u64;

    if abs + 22 > size {
        report.push(Finding::corrupt(
            "ZIP_TRUNCATED",
            "EOCD 记录不完整（文件被截断）",
            Some(abs),
        ));
        return None;
    }

    let comment_len = u16::from_le_bytes([tail[rel + 20], tail[rel + 21]]) as u64;
    let expected_end = abs + 22 + comment_len;
    if expected_end < size {
        report.push(Finding::warning(
            "ZIP_TRAILING_DATA",
            format!("EOCD 之后仍有 {} 字节多余数据", size - expected_end),
            Some(expected_end),
        ));
    } else if expected_end > size {
        report.push(Finding::corrupt(
            "ZIP_TRUNCATED",
            format!("EOCD 注释长度声明 {comment_len} 字节，超出文件末尾（疑似截断）"),
            Some(abs),
        ));
        return None;
    }

    let total_entries = u16::from_le_bytes([tail[rel + 10], tail[rel + 11]]);
    let disk_no = u16::from_le_bytes([tail[rel + 4], tail[rel + 5]]);
    let cd_disk = u16::from_le_bytes([tail[rel + 6], tail[rel + 7]]);
    if disk_no != 0 || cd_disk != 0 {
        report.push(Finding::warning(
            "ZIP_MULTI_DISK",
            format!("多卷压缩包（磁盘号 {disk_no}/{cd_disk}），可能不完整"),
            Some(abs + 4),
        ));
    }

    // 中央目录位置/大小一致性（非 zip64 时）
    let cd_size = u32::from_le_bytes(tail[rel + 12..rel + 16].try_into().unwrap());
    let cd_off = u32::from_le_bytes(tail[rel + 16..rel + 20].try_into().unwrap());
    if total_entries != 0xFFFF
        && cd_size != 0xFFFFFFFF
        && cd_off != 0xFFFFFFFF
        && u64::from(cd_off) + u64::from(cd_size) != abs
    {
        report.push(Finding::corrupt(
            "ZIP_CD_MISMATCH",
            format!(
                "中央目录（偏移 {cd_off}，长度 {cd_size}）结束于 {}，与 EOCD 位置 {abs} 不一致",
                u64::from(cd_off) + u64::from(cd_size)
            ),
            Some(abs),
        ));
    }

    Some((abs, total_entries))
}

/// 用 zip crate 打开归档，逐条读取并比对 CRC
fn verify_archive(
    path: &Path,
    size: u64,
    eocd: Option<(u64, u16)>,
    opts: &Opts,
    report: &mut Report,
) {
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

    let mut archive = match zip::ZipArchive::new(BufReader::new(file)) {
        Ok(a) => a,
        Err(e) => {
            let has_eocd_issue = report
                .findings
                .iter()
                .any(|f| f.code == "ZIP_NO_EOCD" || f.code == "ZIP_TRUNCATED");
            if !has_eocd_issue {
                report.push(Finding::corrupt(
                    "ZIP_BAD_ARCHIVE",
                    format!("无法解析 ZIP 中央目录：{e}"),
                    eocd.map(|(pos, _)| pos),
                ));
            }
            return;
        }
    };

    if let Some((_, total)) = eocd
        && total as usize != archive.len()
    {
        report.push(Finding::warning(
            "ZIP_COUNT_MISMATCH",
            format!("EOCD 记录条目数 {total}，中央目录实际 {} 条", archive.len()),
            eocd.map(|(p, _)| p),
        ));
    }

    let count = archive.len();
    if count == 0 {
        report.push(Finding::warning("ZIP_EMPTY", "压缩包中没有任何条目", None));
        return;
    }

    // 逐条 CRC 检查只对「会做深度检测」的文件执行
    if !opts.should_deep(size) {
        return;
    }

    for idx in 0..count {
        let mut entry = match archive.by_index(idx) {
            Ok(e) => e,
            Err(e) => {
                report.push(Finding::corrupt(
                    "ZIP_BAD_ENTRY",
                    format!("第 {} 个条目无法读取：{e}", idx + 1),
                    None,
                ));
                continue;
            }
        };
        let name = entry.name().to_string();
        let stored_crc = entry.crc32();
        let data_start = entry.data_start();

        let mut hasher = crc32fast::Hasher::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut read_err: Option<std::io::Error> = None;
        loop {
            match entry.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buf[..n]),
                Err(e) => {
                    read_err = Some(e);
                    break;
                }
            }
        }

        if let Some(e) = read_err {
            let msg = e.to_string();
            let low = msg.to_ascii_lowercase();
            if low.contains("checksum") || low.contains("crc") {
                report.push(Finding::corrupt(
                    "ZIP_BAD_CRC",
                    format!("条目 {name} 的 CRC 校验失败（记录 {stored_crc:08x}）"),
                    Some(data_start),
                ));
            } else {
                report.push(Finding::corrupt(
                    "ZIP_DECOMPRESS_FAILED",
                    format!("条目 {name} 解压失败：{msg}"),
                    Some(data_start),
                ));
            }
            continue;
        }

        let computed = hasher.finalize();
        if computed != stored_crc {
            report.push(Finding::corrupt(
                "ZIP_BAD_CRC",
                format!("条目 {name} 的 CRC 不匹配：记录 {stored_crc:08x}，实际 {computed:08x}"),
                Some(data_start),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// 公共小工具
// ---------------------------------------------------------------------------

/// 打开文件并读取 [off, off+len)（调用方需预先保证范围在文件内）
pub(crate) fn read_at(f: &mut File, off: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

pub(crate) fn file_size(path: &Path, report: &mut Report) -> Option<u64> {
    match std::fs::metadata(path) {
        Ok(m) => Some(m.len()),
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("无法读取文件：{e}"),
                None,
            ));
            None
        }
    }
}

pub(crate) fn open_file(path: &Path, report: &mut Report) -> Option<File> {
    match File::open(path) {
        Ok(f) => Some(f),
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("打开文件失败：{e}"),
                None,
            ));
            None
        }
    }
}

pub(crate) fn read_range(
    path: &Path,
    off: u64,
    len: usize,
    report: &mut Report,
    what: &str,
) -> Option<Vec<u8>> {
    let mut f = open_file(path, report)?;
    match read_at(&mut f, off, len) {
        Ok(buf) => Some(buf),
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("读取{what}失败：{e}"),
                Some(off),
            ));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// gzip
// ---------------------------------------------------------------------------

/// 解压体积上限：超过则只做头部校验
const MAX_DECODED_BYTES: u64 = 512 * 1024 * 1024;

pub struct GzipVerifier;

impl Verifier for GzipVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Gzip)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Gzip))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "gzip", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        let full = size <= 1 << 20;
        let head_len = size.min(1 << 20) as usize;
        let Some(head) = read_range(path, 0, head_len, &mut report, "gzip 头") else {
            return report;
        };
        if parse_gzip_header(&head, full, &mut report).is_none() {
            return report;
        }
        if opts.should_deep(size) {
            decode_gzip(path, &mut report);
        }
        report
    }
}

/// 解析 gzip 固定头与可选字段，返回压缩数据起始偏移；出错时记结论并返回 None
fn parse_gzip_header(head: &[u8], full: bool, report: &mut Report) -> Option<usize> {
    if head.len() < 10 {
        report.push(Finding::corrupt(
            "GZIP_TRUNCATED",
            format!("gzip 固定头不足 10 字节（仅 {} 字节）", head.len()),
            Some(0),
        ));
        return None;
    }
    if head[0] != 0x1F || head[1] != 0x8B {
        report.push(Finding::corrupt(
            "GZIP_BAD_SIG",
            "缺少 gzip 魔数 1F 8B",
            Some(0),
        ));
        return None;
    }
    if head[2] != 8 {
        report.push(Finding::corrupt(
            "GZIP_BAD_CM",
            format!("压缩方法为 {}（应为 8/deflate）", head[2]),
            Some(2),
        ));
        return None;
    }
    let flg = head[3];
    if flg & 0xE0 != 0 {
        report.push(Finding::corrupt(
            "GZIP_BAD_FLAGS",
            format!("FLG 含保留位：{flg:#04x}"),
            Some(3),
        ));
        return None;
    }

    let mut i = 10usize;
    if flg & 0x04 != 0 {
        if i + 2 > head.len() {
            report.push(Finding::corrupt(
                "GZIP_TRUNCATED",
                "FEXTRA 长度字段缺失",
                Some(i as u64),
            ));
            return None;
        }
        let xlen = usize::from(u16::from_le_bytes([head[i], head[i + 1]]));
        i += 2;
        if i + xlen > head.len() {
            report.push(Finding::corrupt(
                "GZIP_TRUNCATED",
                format!("FEXTRA 声明 {xlen} 字节，超出文件末尾"),
                Some(i as u64),
            ));
            return None;
        }
        i += xlen;
    }
    if flg & 0x08 != 0 && !skip_gzip_cstr(head, &mut i, "文件名", full, report) {
        return None;
    }
    if flg & 0x10 != 0 && !skip_gzip_cstr(head, &mut i, "注释", full, report) {
        return None;
    }
    if flg & 0x02 != 0 {
        if i + 2 > head.len() {
            report.push(Finding::corrupt(
                "GZIP_TRUNCATED",
                "FHCRC 字段缺失",
                Some(i as u64),
            ));
            return None;
        }
        let want = u16::from_le_bytes([head[i], head[i + 1]]);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&head[..i]);
        if (hasher.finalize() & 0xFFFF) as u16 != want {
            report.push(Finding::corrupt(
                "GZIP_BAD_FHCRC",
                format!("头部 CRC16 不匹配：记录 {want:04x}"),
                Some(i as u64),
            ));
            return None;
        }
        i += 2;
    }
    if i >= head.len() {
        report.push(Finding::corrupt(
            "GZIP_TRUNCATED",
            "头部之后没有压缩数据",
            Some(i as u64),
        ));
        return None;
    }
    Some(i)
}

/// 跳过以 NUL 结尾的字符串字段（FNAME/FCOMMENT）
fn skip_gzip_cstr(
    head: &[u8],
    i: &mut usize,
    what: &str,
    full: bool,
    report: &mut Report,
) -> bool {
    match head[*i..].iter().position(|&b| b == 0) {
        Some(p) => {
            *i += p + 1;
            true
        }
        None => {
            if full {
                report.push(Finding::corrupt(
                    "GZIP_TRUNCATED",
                    format!("gzip{what}缺少 NUL 结尾"),
                    Some(*i as u64),
                ));
            } else {
                report.push(Finding::warning(
                    "GZIP_HEADER_TOO_LONG",
                    format!("gzip{what}字段超过 1 MiB，跳过深度校验"),
                    Some(*i as u64),
                ));
            }
            false
        }
    }
}

/// 用 MultiGzDecoder 整流解码，按错误文本分类
fn decode_gzip(path: &Path, report: &mut Report) {
    use std::io::Read;

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
    let mut dec = flate2::read::MultiGzDecoder::new(BufReader::new(file));
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        match dec.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > MAX_DECODED_BYTES {
                    report.push(Finding::unknown(
                        "GZIP_TOO_LARGE",
                        format!(
                            "解压超过 {} MiB，停止深度校验",
                            MAX_DECODED_BYTES / (1024 * 1024)
                        ),
                        None,
                    ));
                    return;
                }
            }
            Err(e) => {
                let low = e.to_string().to_ascii_lowercase();
                let (code, msg) = if low.contains("checksum") {
                    ("GZIP_BAD_CRC", format!("gzip 末尾 CRC32 校验失败：{e}"))
                } else if low.contains("length") {
                    ("GZIP_BAD_ISIZE", format!("gzip 末尾 ISIZE 长度不匹配：{e}"))
                } else if low.contains("not in gzip format") {
                    ("GZIP_BAD_HEADER", format!("gzip 头解析失败：{e}"))
                } else if low.contains("eof")
                    || low.contains("end of file")
                    || low.contains("fill whole")
                {
                    ("GZIP_TRUNCATED", format!("gzip 数据提前结束：{e}"))
                } else {
                    ("GZIP_DEFLATE_FAILED", format!("deflate 解压失败：{e}"))
                };
                report.push(Finding::corrupt(code, msg, None));
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// tar
// ---------------------------------------------------------------------------

pub struct TarVerifier;

impl Verifier for TarVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Tar)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Tar))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "tar", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        let Some(f) = open_file(path, &mut report) else {
            return report;
        };
        verify_tar(f, size, &mut report);
        report
    }
}

fn verify_tar(mut f: File, size: u64, report: &mut Report) {
    let mut off = 0u64;
    let mut entries = 0u64;
    let mut end_seen = false;

    while off < size {
        if size - off < 512 {
            report.push(Finding::corrupt(
                "TAR_TRUNCATED",
                format!("偏移 {off} 处只剩 {} 字节，不足一个 512 字节块", size - off),
                Some(off),
            ));
            return;
        }
        let block = match read_at(&mut f, off, 512) {
            Ok(b) => b,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("读取归档失败：{e}"),
                    Some(off),
                ));
                return;
            }
        };

        if block.iter().all(|&b| b == 0) {
            end_seen = true;
            if off + 512 < size
                && let Ok(next) = read_at(&mut f, off + 512, 512)
                && next.iter().any(|&b| b != 0)
            {
                report.push(Finding::warning(
                    "TAR_BAD_END_MARK",
                    format!("偏移 {} 的全零结束块之后仍有数据", off + 512),
                    Some(off + 512),
                ));
            }
            break;
        }

        // 校验和：和值对整个 512 字节块计算，校验和字段按空格处理
        let stored = parse_octal(&block[148..156]);
        let (mut sum_u, mut sum_s) = (0u64, 0i64);
        for (i, &b) in block.iter().enumerate() {
            let byte = if (148..156).contains(&i) { b' ' } else { b };
            sum_u += u64::from(byte);
            sum_s += i64::from(byte as i8);
        }
        match stored {
            Some(s) if s == sum_u || i64::try_from(s) == Ok(sum_s) => {}
            Some(s) => {
                report.push(Finding::corrupt(
                    "TAR_BAD_CHECKSUM",
                    format!(
                        "偏移 {off} 处块校验和不匹配：记录 {s:o}，计算 {sum_u:o}(无符号)/{sum_s:o}(有符号)"
                    ),
                    Some(off + 148),
                ));
                return;
            }
            None => {
                report.push(Finding::corrupt(
                    "TAR_BAD_CHECKSUM",
                    format!("偏移 {off} 处校验和字段不是合法八进制"),
                    Some(off + 148),
                ));
                return;
            }
        }

        let Some(sz) = parse_tar_size(&block[124..136]) else {
            report.push(Finding::corrupt(
                "TAR_BAD_SIZE",
                format!("偏移 {off} 处条目大小字段不是合法数值"),
                Some(off + 124),
            ));
            return;
        };
        if !tar_type_valid(block[156]) {
            report.push(Finding::warning(
                "TAR_BAD_TYPE",
                format!("偏移 {off} 处未知类型标志 {:#04x}", block[156]),
                Some(off + 156),
            ));
        }
        entries += 1;

        let Some(data_end) = off.checked_add(512).and_then(|v| v.checked_add(sz)) else {
            report.push(Finding::corrupt(
                "TAR_BAD_SIZE",
                format!("偏移 {off} 处条目大小溢出"),
                Some(off + 124),
            ));
            return;
        };
        if data_end > size {
            report.push(Finding::corrupt(
                "TAR_TRUNCATED",
                format!("偏移 {off} 的条目数据（{sz} 字节）超出文件末尾 {size}"),
                Some(off + 124),
            ));
            return;
        }
        let padded = match sz.checked_add(511) {
            Some(v) => v / 512 * 512,
            None => {
                report.push(Finding::corrupt(
                    "TAR_BAD_SIZE",
                    format!("偏移 {off} 处条目大小溢出"),
                    Some(off + 124),
                ));
                return;
            }
        };
        let Some(next) = off.checked_add(512).and_then(|v| v.checked_add(padded)) else {
            report.push(Finding::corrupt(
                "TAR_BAD_SIZE",
                format!("偏移 {off} 处条目大小溢出"),
                Some(off + 124),
            ));
            return;
        };
        off = next;
    }

    if entries == 0 {
        report.push(Finding::warning("TAR_EMPTY", "归档中没有任何条目", None));
    }
    if !end_seen {
        report.push(Finding::warning(
            "TAR_NO_END",
            "未找到全零结束块（512 字节），归档可能被截断",
            None,
        ));
    }
}

/// 八进制字段（允许前导空格/NUL，遇到终止符停止）
fn parse_octal(field: &[u8]) -> Option<u64> {
    let mut val = 0u64;
    let mut started = false;
    for &b in field {
        if !started && (b == b' ' || b == b'\0') {
            continue;
        }
        if b == b' ' || b == b'\0' {
            break;
        }
        if !b.is_ascii_digit() || b > b'7' {
            return None;
        }
        started = true;
        val = val.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
    }
    started.then_some(val)
}

/// 条目大小：普通八进制或 GNU base-256（首字节最高位置位）
fn parse_tar_size(field: &[u8]) -> Option<u64> {
    let &first = field.first()?;
    if first & 0x80 != 0 {
        if first == 0xFF {
            return None; // 负值，非法
        }
        let mut val = u64::from(first & 0x7F);
        for &b in &field[1..] {
            val = val.checked_mul(256)?.checked_add(u64::from(b))?;
        }
        Some(val)
    } else {
        parse_octal(field)
    }
}

fn tar_type_valid(flag: u8) -> bool {
    matches!(
        flag,
        0 | b'0'..=b'7' | b'A' | b'D' | b'K' | b'L' | b'M' | b'N' | b'S' | b'V' | b'g' | b'x'
    )
}

// ---------------------------------------------------------------------------
// 7z
// ---------------------------------------------------------------------------

const SEVENZ_SIG: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

pub struct SevenZVerifier;

impl Verifier for SevenZVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::SevenZ)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::SevenZ))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "7z", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size < 32 {
            report.push(Finding::corrupt(
                "SEVENZ_TRUNCATED",
                format!("7z 签名头需 32 字节，实际只有 {size} 字节"),
                Some(0),
            ));
            return report;
        }
        let Some(head) = read_range(path, 0, 32, &mut report, "7z 签名头") else {
            return report;
        };
        if head[..6] != SEVENZ_SIG {
            report.push(Finding::corrupt(
                "SEVENZ_BAD_SIG",
                "缺少 7z 魔数 37 7A BC AF 27 1C",
                Some(0),
            ));
            return report;
        }
        if head[6] != 0 {
            report.push(Finding::warning(
                "SEVENZ_BAD_VERSION",
                format!("主版本号为 {}（期望 0）", head[6]),
                Some(6),
            ));
        }

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&head[12..32]);
        let start_crc = u32::from_le_bytes(head[8..12].try_into().unwrap());
        if hasher.finalize() != start_crc {
            report.push(Finding::corrupt(
                "SEVENZ_BAD_START_CRC",
                format!("StartHeaderCRC 不匹配（记录 {start_crc:08x}）"),
                Some(8),
            ));
            return report;
        }

        let next_off = u64::from_le_bytes(head[12..20].try_into().unwrap());
        let next_size = u64::from_le_bytes(head[20..28].try_into().unwrap());
        let next_crc = u32::from_le_bytes(head[28..32].try_into().unwrap());

        let Some(hdr_start) = 32u64.checked_add(next_off) else {
            report.push(Finding::corrupt(
                "SEVENZ_BAD_OFFSET",
                "后续头偏移溢出",
                Some(12),
            ));
            return report;
        };
        if next_size == 0 {
            if hdr_start < size {
                report.push(Finding::warning(
                    "SEVENZ_TRAILING_DATA",
                    format!("未声明后续头，文件剩余 {} 字节", size - hdr_start),
                    Some(hdr_start),
                ));
            } else {
                report.push(Finding::warning(
                    "SEVENZ_EMPTY_HEADER",
                    "7z 未声明任何后续头内容（空归档？）",
                    None,
                ));
            }
            return report;
        }
        if next_size > 64 * 1024 * 1024 {
            report.push(Finding::warning(
                "SEVENZ_HEADER_TOO_LARGE",
                format!("7z 后续头声明 {next_size} 字节，过大而跳过校验"),
                Some(hdr_start),
            ));
            return report;
        }
        let Some(hdr_end) = hdr_start.checked_add(next_size) else {
            report.push(Finding::corrupt(
                "SEVENZ_BAD_OFFSET",
                "后续头位置溢出",
                Some(12),
            ));
            return report;
        };
        if hdr_end > size {
            report.push(Finding::corrupt(
                "SEVENZ_TRUNCATED",
                format!(
                    "后续头（偏移 {hdr_start}，长 {next_size} 字节）超出文件末尾 {size}"
                ),
                Some(hdr_start),
            ));
            return report;
        }

        let Some(hdr) = read_range(
            path,
            hdr_start,
            next_size as usize,
            &mut report,
            "7z 后续头",
        ) else {
            return report;
        };
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&hdr);
        let computed = hasher.finalize();
        if computed != next_crc {
            report.push(Finding::corrupt(
                "SEVENZ_BAD_HEADER_CRC",
                format!("NextHeaderCRC 不匹配：记录 {next_crc:08x}，实际 {computed:08x}"),
                Some(hdr_start),
            ));
            return report;
        }
        if hdr_end < size {
            report.push(Finding::warning(
                "SEVENZ_TRAILING_DATA",
                format!("7z 后续头之后仍有 {} 字节未声明数据", size - hdr_end),
                Some(hdr_end),
            ));
        }
        report
    }
}

// ---------------------------------------------------------------------------
// rar（RAR4 / RAR5）
// ---------------------------------------------------------------------------

const RAR4_SIG: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const RAR5_SIG: [u8; 8] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];

pub struct RarVerifier;

impl Verifier for RarVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Rar)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Rar))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "RAR", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size < 7 {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("RAR 签名至少 7 字节，实际只有 {size} 字节"),
                Some(0),
            ));
            return report;
        }
        let head_len = size.min(8) as usize;
        let Some(head) = read_range(path, 0, head_len, &mut report, "RAR 签名") else {
            return report;
        };

        let walked = if head.starts_with(&RAR4_SIG) {
            open_file(path, &mut report).and_then(|mut f| verify_rar4(&mut f, size, &mut report))
        } else if head.starts_with(&RAR5_SIG) {
            open_file(path, &mut report).and_then(|mut f| verify_rar5(&mut f, size, &mut report))
        } else {
            report.push(Finding::corrupt(
                "RAR_BAD_SIG",
                "缺少 RAR 签名（Rar!\\x1a\\x07）",
                Some(0),
            ));
            None
        };
        if walked == Some(false) {
            report.push(Finding::warning(
                "RAR_NO_END",
                "未找到结束块，归档可能不完整",
                None,
            ));
        }
        report
    }
}

/// RAR4 合法块类型：0x72..0x7B
fn rar4_type(t: u8) -> bool {
    matches!(t, 0x72..=0x7B)
}

/// RAR4 走块：HEAD_CRC 为块体（type 起）CRC32 的低 16 位。
/// 布局上兼容 4 字节 CRC（规范）与 2 字节 CRC（实测 WinRAR 输出）两种块头。
/// 返回 Some(是否见到结束块)，None 表示已记下致命结论。
fn verify_rar4(f: &mut File, size: u64, report: &mut Report) -> Option<bool> {
    let mut off = 7u64; // 签名之后
    let mut first = true;
    loop {
        if off >= size {
            break;
        }
        let avail = (size - off) as usize;
        let peek = match read_at(f, off, avail.min(9)) {
            Ok(p) => p,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("读取 RAR4 块头失败：{e}"),
                    Some(off),
                ));
                return None;
            }
        };
        // 优先按 4 字节 CRC 布局解析；不成立时回退到 2 字节 CRC 布局
        let layout = if peek.len() >= 9
            && rar4_type(peek[4])
            && u16::from_le_bytes([peek[7], peek[8]]) >= 9
        {
            Some((4usize, 4usize))
        } else if peek.len() >= 7
            && rar4_type(peek[2])
            && u16::from_le_bytes([peek[5], peek[6]]) >= 7
        {
            Some((2usize, 2usize))
        } else {
            None
        };
        let Some((crc_len, type_off)) = layout else {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处块头不完整或无法识别（余 {avail} 字节）"),
                Some(off),
            ));
            return None;
        };

        let btype = peek[type_off];
        let flags = u16::from_le_bytes([peek[type_off + 1], peek[type_off + 2]]);
        let hsize = u64::from(u16::from_le_bytes([peek[type_off + 3], peek[type_off + 4]]));
        if off + hsize > size {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处块声明长度 {hsize} 超出文件末尾 {size}"),
                Some(off),
            ));
            return None;
        }
        let hdr = match read_at(f, off, hsize as usize) {
            Ok(h) => h,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("读取 RAR4 块失败：{e}"),
                    Some(off),
                ));
                return None;
            }
        };
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&hdr[crc_len..]);
        let computed = hasher.finalize();
        let stored = u16::from_le_bytes([hdr[0], hdr[1]]);
        if (computed & 0xFFFF) as u16 != stored {
            report.push(Finding::corrupt(
                "RAR_BAD_CRC",
                format!(
                    "偏移 {off} 处块头 CRC 不匹配：记录 {stored:04x}，计算 {:04x}",
                    computed & 0xFFFF
                ),
                Some(off),
            ));
            return None;
        }

        if first {
            if btype != 0x73 {
                report.push(Finding::warning(
                    "RAR_NO_MAIN",
                    format!("首个块类型 {btype:#04x}，不是主头 (0x73)"),
                    Some(off),
                ));
            }
            first = false;
        }
        if btype == 0x7B {
            let end = off + hsize;
            if end < size {
                report.push(Finding::warning(
                    "RAR_TRAILING_DATA",
                    format!("结束块之后仍有 {} 字节数据", size - end),
                    Some(end),
                ));
            }
            return Some(true);
        }

        // LONG_BLOCK(0x8000) 时，头后紧跟 4 字节 ADD_SIZE（FILE_HEAD 即压缩数据长度）
        let add = if flags & 0x8000 != 0 {
            let p = crc_len + 5;
            if p + 4 > hdr.len() {
                report.push(Finding::corrupt(
                    "RAR_BAD_BLOCK",
                    format!("偏移 {off} 处块声明有数据但缺少 ADD_SIZE 字段"),
                    Some(off),
                ));
                return None;
            }
            u64::from(u32::from_le_bytes(hdr[p..p + 4].try_into().unwrap()))
        } else {
            0
        };
        let Some(next) = off.checked_add(hsize).and_then(|v| v.checked_add(add)) else {
            report.push(Finding::corrupt(
                "RAR_BAD_BLOCK",
                format!("偏移 {off} 处块长度溢出"),
                Some(off),
            ));
            return None;
        };
        if next > size {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处块数据（头 {hsize} + 数据 {add}）超出文件末尾 {size}"),
                Some(off),
            ));
            return None;
        }
        off = next;
    }
    Some(false)
}

/// RAR5 变长整数（7 位小端），返回 (占用字节数, 值)
fn read_vint(bytes: &[u8]) -> Option<(usize, u64)> {
    let mut val = 0u64;
    for (i, &b) in bytes.iter().enumerate() {
        if i >= 10 {
            return None;
        }
        let shift = i * 7;
        if shift >= 64 || u64::from(b & 0x7F) > (u64::MAX >> shift) {
            return None;
        }
        val |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Some((i + 1, val));
        }
    }
    None // 被截断的 vint
}

/// RAR5 走块：块头 CRC 为 [size 字段起点..头末] 的完整 CRC32。
/// 返回 Some(是否见到结束块)，None 表示已记下致命结论。
fn verify_rar5(f: &mut File, size: u64, report: &mut Report) -> Option<bool> {
    let mut off = 8u64; // 签名之后
    let mut first = true;
    loop {
        if off >= size {
            break;
        }
        if size - off < 5 {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处只剩 {} 字节，不足一个 RAR5 块头", size - off),
                Some(off),
            ));
            return None;
        }
        let crc_bytes = match read_at(f, off, 4) {
            Ok(c) => c,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("读取 RAR5 块 CRC 失败：{e}"),
                    Some(off),
                ));
                return None;
            }
        };
        let stored = u32::from_le_bytes(crc_bytes.try_into().unwrap());

        let avail = (size - off - 4) as usize;
        let vsnip = match read_at(f, off + 4, avail.min(10)) {
            Ok(v) => v,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("读取 RAR5 块长度失败：{e}"),
                    Some(off + 4),
                ));
                return None;
            }
        };
        let Some((vlen, hsize)) = read_vint(&vsnip) else {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处块长度字段被截断"),
                Some(off + 4),
            ));
            return None;
        };
        let Some(body_end) = off
            .checked_add(4)
            .and_then(|v| v.checked_add(vlen as u64))
            .and_then(|v| v.checked_add(hsize))
        else {
            report.push(Finding::corrupt(
                "RAR_BAD_BLOCK",
                format!("偏移 {off} 处块长度溢出"),
                Some(off),
            ));
            return None;
        };
        if body_end > size {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处块声明长度 {hsize} 超出文件末尾 {size}"),
                Some(off),
            ));
            return None;
        }

        let crc_len = (body_end - off - 4) as usize;
        let crc_data = match read_at(f, off + 4, crc_len) {
            Ok(d) => d,
            Err(e) => {
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("读取 RAR5 块体失败：{e}"),
                    Some(off + 4),
                ));
                return None;
            }
        };
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&crc_data);
        let computed = hasher.finalize();
        if computed != stored {
            report.push(Finding::corrupt(
                "RAR_BAD_CRC",
                format!("偏移 {off} 处块头 CRC 不匹配：记录 {stored:08x}，计算 {computed:08x}"),
                Some(off),
            ));
            return None;
        }

        // 块体：type, flags, [extra_size], [data_size], ...类型相关字段
        let body = &crc_data[vlen..];
        let mut p = 0usize;
        let next_field = |p: usize| body.get(p..).and_then(read_vint);
        let Some((n, btype)) = next_field(p) else {
            report.push(Finding::corrupt(
                "RAR_BAD_BLOCK",
                format!("偏移 {off} 处块类型字段损坏"),
                Some(off),
            ));
            return None;
        };
        p += n;
        let Some((n, flags)) = next_field(p) else {
            report.push(Finding::corrupt(
                "RAR_BAD_BLOCK",
                format!("偏移 {off} 处块标志字段损坏"),
                Some(off),
            ));
            return None;
        };
        p += n;
        if flags & 0x1 != 0 {
            let Some((n, _extra)) = next_field(p) else {
                report.push(Finding::corrupt(
                    "RAR_BAD_BLOCK",
                    format!("偏移 {off} 处扩展区长度字段损坏"),
                    Some(off),
                ));
                return None;
            };
            p += n;
        }
        let mut data_len = 0u64;
        if flags & 0x2 != 0 {
            let Some((_, d)) = next_field(p) else {
                report.push(Finding::corrupt(
                    "RAR_BAD_BLOCK",
                    format!("偏移 {off} 处数据长度字段损坏"),
                    Some(off),
                ));
                return None;
            };
            data_len = d;
        }

        if first {
            if btype != 1 {
                report.push(Finding::warning(
                    "RAR_NO_MAIN",
                    format!("首个块类型 {btype}，不是主头 (1)"),
                    Some(off),
                ));
            }
            first = false;
        }

        let Some(next) = body_end.checked_add(data_len) else {
            report.push(Finding::corrupt(
                "RAR_BAD_BLOCK",
                format!("偏移 {off} 处数据长度溢出"),
                Some(off),
            ));
            return None;
        };
        if next > size {
            report.push(Finding::corrupt(
                "RAR_TRUNCATED",
                format!("偏移 {off} 处块数据（{data_len} 字节）超出文件末尾 {size}"),
                Some(off),
            ));
            return None;
        }
        if btype == 5 {
            // 结束块
            if next < size {
                report.push(Finding::warning(
                    "RAR_TRAILING_DATA",
                    format!("结束块之后仍有 {} 字节数据", size - next),
                    Some(next),
                ));
            }
            return Some(true);
        }
        off = next;
    }
    Some(false)
}
