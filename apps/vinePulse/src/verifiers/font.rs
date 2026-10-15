//! 字体验证器：TrueType/OpenType（sfnt）、字体集合（TTC）、WOFF/WOFF2
//!
//! 结构遍历：表目录边界、表数据范围、WOFF 头/目录自洽；
//! 深度模式下额外校验 sfnt 表求和校验和（不符仅记 warning——
//! 部分生成工具不写校验和，不宜判损坏）。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Verifier};

pub struct FontVerifier;

impl Verifier for FontVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Font)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Font))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "Font", Mode::Blind);
        verify_font(path, opts, &mut report);
        report
    }
}

fn open_size(path: &Path, report: &mut Report) -> Option<(File, u64)> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("打开文件失败：{e}"), None));
            return None;
        }
    };
    let size = match f.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("读取文件大小失败：{e}"), None));
            return None;
        }
    };
    Some((f, size))
}

fn verify_font(path: &Path, opts: &Opts, report: &mut Report) {
    let Some((file, size)) = open_size(path, report) else {
        return;
    };
    if size < 4 {
        report.push(Finding::corrupt(
            "FONT_TRUNCATED",
            format!("文件仅 {size} 字节，不足字体签名"),
            Some(0),
        ));
        return;
    }
    let mut r = BufReader::new(file);
    let mut sig = [0u8; 4];
    if r.read_exact(&mut sig).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取字体签名失败", Some(0)));
        return;
    }
    match &sig {
        b"wOFF" => {
            report.file_type = "WOFF".to_string();
            verify_woff1(&mut r, size, report);
        }
        b"wOF2" => {
            report.file_type = "WOFF2".to_string();
            verify_woff2(&mut r, size, report);
        }
        b"ttcf" => {
            report.file_type = "TrueType Collection".to_string();
            verify_ttc(&mut r, size, report);
        }
        [0x00, 0x01, 0x00, 0x00] => {
            report.file_type = "TrueType".to_string();
            verify_sfnt(&mut r, 0, size, size, opts, report, true);
        }
        b"OTTO" => {
            report.file_type = "OpenType".to_string();
            verify_sfnt(&mut r, 0, size, size, opts, report, true);
        }
        b"true" | b"typ1" => {
            report.file_type = "TrueType".to_string();
            verify_sfnt(&mut r, 0, size, size, opts, report, true);
        }
        _ => {
            report.push(Finding::corrupt(
                "FONT_BAD_FLAVOR",
                format!("未知的字体签名 {sig:02x?}"),
                Some(0),
            ));
        }
    }
}

/// sfnt 头（12 字节）+ 表目录（16 字节/表）+ 各表数据范围
///
/// `base`：sfnt 头在文件中的偏移（TTC 内部字体非 0）
/// `check_sums`：TTC 内字体共享数据，跳过全量求和校验
fn verify_sfnt(
    r: &mut BufReader<File>,
    base: u64,
    size: u64,
    file_size: u64,
    opts: &Opts,
    report: &mut Report,
    check_sums: bool,
) {
    let mut hdr = [0u8; 12];
    if r.seek(SeekFrom::Start(base)).is_err() || r.read_exact(&mut hdr).is_err() {
        report.push(Finding::corrupt(
            "FONT_TRUNCATED",
            format!("偏移 {base} 处读不到 sfnt 头"),
            Some(base),
        ));
        return;
    }
    if hdr[0..4] != [0x00, 0x01, 0x00, 0x00] && hdr[0..4] != *b"OTTO" && hdr[0..4] != *b"true"
    {
        report.push(Finding::corrupt(
            "FONT_BAD_FLAVOR",
            format!("sfnt version 为 {:02x?}（应为 00010000 / OTTO / true）", &hdr[0..4]),
            Some(base),
        ));
        return;
    }
    let num_tables = u16::from_be_bytes([hdr[4], hdr[5]]);
    if num_tables == 0 {
        report.push(Finding::corrupt(
            "FONT_NO_TABLES",
            "numTables 为 0（没有表的字体无效）",
            Some(base + 4),
        ));
        return;
    }
    let dir_end = base + 12 + u64::from(num_tables) * 16;
    if dir_end > size {
        report.push(Finding::corrupt(
            "FONT_TABLE_DIRECTORY_TRUNCATED",
            format!(
                "表目录 {num_tables} 项需要到偏移 {dir_end}，超出文件范围 {size}"
            ),
            Some(base + 4),
        ));
        return;
    }

    let mut bad_checksums = 0u32;
    let do_check = check_sums && opts.core_deep(file_size);
    for i in 0..num_tables {
        let mut entry = [0u8; 16];
        let at = base + 12 + u64::from(i) * 16;
        if r.seek(SeekFrom::Start(at)).is_err() || r.read_exact(&mut entry).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取表目录项失败", Some(at)));
            return;
        }
        let tag = &entry[0..4];
        if !tag.iter().all(|b| b.is_ascii() && !b.is_ascii_control()) {
            report.push(Finding::warning(
                "FONT_BAD_TAG",
                format!("表 {i} 标签 {tag:02x?} 含非 ASCII 字符"),
                Some(at),
            ));
        }
        let checksum = u32::from_be_bytes(entry[4..8].try_into().unwrap());
        let offset = u32::from_be_bytes(entry[8..12].try_into().unwrap());
        let length = u32::from_be_bytes(entry[12..16].try_into().unwrap());
        let start = base + u64::from(offset);
        if u64::from(length) > size || start.checked_add(u64::from(length)).is_none_or(|e| e > size)
        {
            report.push(Finding::corrupt(
                "FONT_TABLE_OUT_OF_RANGE",
                format!(
                    "表 {}（{:?}）数据 offset={offset} length={length}，超出文件范围 {size}",
                    i,
                    String::from_utf8_lossy(tag)
                ),
                Some(at + 8),
            ));
            return;
        }
        if do_check
            && checksum != 0
            && table_checksum(r, start, length).is_some_and(|actual| actual != checksum)
        {
            bad_checksums += 1;
        }
    }
    if bad_checksums > 0 {
        report.push(Finding::warning(
            "FONT_TABLE_CHECKSUM_MISMATCH",
            format!("{bad_checksums} 个表的校验和与内容不符"),
            None,
        ));
    }
}

/// 表数据的 sfnt 求和校验和：以大端 u32 累加，不足 4 字节用 0 补齐
fn table_checksum(r: &mut BufReader<File>, start: u64, length: u32) -> Option<u32> {
    r.seek(SeekFrom::Start(start)).ok()?;
    let mut sum = 0u32;
    let mut remaining = length as usize;
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len());
        r.read_exact(&mut buf[..want]).ok()?;
        let (chunks, tail) = buf[..want].as_chunks::<4>();
        for chunk in chunks {
            sum = sum.wrapping_add(u32::from_be_bytes(*chunk));
        }
        if !tail.is_empty() {
            // 尾部不足 4 字节：补 0 后按大端读
            let mut last = [0u8; 4];
            last[..tail.len()].copy_from_slice(tail);
            sum = sum.wrapping_add(u32::from_be_bytes(last));
        }
        remaining -= want;
    }
    Some(sum)
}

/// WOFF：44 字节头 + 20 字节/表目录 + 连续表数据
fn verify_woff1(r: &mut BufReader<File>, size: u64, report: &mut Report) {
    let mut hdr = [0u8; 44];
    if size < 44 || r.seek(SeekFrom::Start(0)).is_err() || r.read_exact(&mut hdr).is_err() {
        report.push(Finding::corrupt(
            "FONT_TRUNCATED",
            format!("文件 {size} 字节，不足 WOFF 头（44 字节）"),
            Some(0),
        ));
        return;
    }
    let length = u32::from_be_bytes(hdr[8..12].try_into().unwrap());
    if u64::from(length) != size {
        report.push(Finding::corrupt(
            "FONT_BAD_LENGTH",
            format!("头部声明长度 {length} 字节，文件实际 {size} 字节"),
            Some(8),
        ));
        return;
    }
    let num_tables = u16::from_be_bytes(hdr[12..14].try_into().unwrap());
    if num_tables == 0 {
        report.push(Finding::corrupt("FONT_NO_TABLES", "numTables 为 0", Some(12)));
        return;
    }
    let dir_end = 44u64 + u64::from(num_tables) * 20;
    if dir_end > size {
        report.push(Finding::corrupt(
            "FONT_TABLE_DIRECTORY_TRUNCATED",
            format!("WOFF 目录 {num_tables} 项需要到偏移 {dir_end}，超出文件范围"),
            Some(12),
        ));
        return;
    }
    let mut entry = [0u8; 20];
    for i in 0..num_tables {
        let at = 44u64 + u64::from(i) * 20;
        if r.seek(SeekFrom::Start(at)).is_err() || r.read_exact(&mut entry).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取 WOFF 目录项失败", Some(at)));
            return;
        }
        let offset = u32::from_be_bytes(entry[8..12].try_into().unwrap());
        let comp = u32::from_be_bytes(entry[12..16].try_into().unwrap());
        let orig = u32::from_be_bytes(entry[16..20].try_into().unwrap());
        if u64::from(offset).checked_add(u64::from(comp)).is_none_or(|e| e > size) {
            report.push(Finding::corrupt(
                "FONT_TABLE_OUT_OF_RANGE",
                format!("表 {i} 数据 offset={offset} compLength={comp}，超出文件范围"),
                Some(at + 8),
            ));
            return;
        }
        if comp > orig {
            report.push(Finding::corrupt(
                "FONT_BAD_COMP_LENGTH",
                format!("表 {i} 压缩后 {comp} 字节大于原始 {orig} 字节"),
                Some(at + 12),
            ));
            return;
        }
    }
}

/// WOFF2：48 字节头；表数据整体压缩存放在目录之后
fn verify_woff2(r: &mut BufReader<File>, size: u64, report: &mut Report) {
    let mut hdr = [0u8; 48];
    if size < 48 || r.seek(SeekFrom::Start(0)).is_err() || r.read_exact(&mut hdr).is_err() {
        report.push(Finding::corrupt(
            "FONT_TRUNCATED",
            format!("文件 {size} 字节，不足 WOFF2 头（48 字节）"),
            Some(0),
        ));
        return;
    }
    let length = u32::from_be_bytes(hdr[8..12].try_into().unwrap());
    if u64::from(length) != size {
        report.push(Finding::corrupt(
            "FONT_BAD_LENGTH",
            format!("头部声明长度 {length} 字节，文件实际 {size} 字节"),
            Some(8),
        ));
        return;
    }
    let num_tables = u16::from_be_bytes(hdr[12..14].try_into().unwrap());
    if num_tables == 0 {
        report.push(Finding::corrupt("FONT_NO_TABLES", "numTables 为 0", Some(12)));
        return;
    }
    let total_compressed = u32::from_be_bytes(hdr[16..20].try_into().unwrap());
    // 压缩数据起点未知（变长目录），下界：头 + 数据 ≤ 文件
    if 48u64.checked_add(u64::from(total_compressed)).is_none_or(|e| e > size) {
        report.push(Finding::corrupt(
            "FONT_DATA_OUT_OF_RANGE",
            format!(
                "totalCompressedSize={total_compressed}，头 48 字节之外放不下（文件 {size} 字节）"
            ),
            Some(16),
        ));
    }
}

/// WOFF2/集合内的辅助结构不在此展开——TTC 仅校验目录与各字体 sfnt 边界
fn verify_ttc(r: &mut BufReader<File>, size: u64, report: &mut Report) {
    let mut hdr = [0u8; 12];
    if size < 12 || r.seek(SeekFrom::Start(0)).is_err() || r.read_exact(&mut hdr).is_err() {
        report.push(Finding::corrupt(
            "FONT_TRUNCATED",
            format!("文件 {size} 字节，不足 TTC 头（12 字节）"),
            Some(0),
        ));
        return;
    }
    let num_fonts = u32::from_be_bytes(hdr[8..12].try_into().unwrap());
    if num_fonts == 0 {
        report.push(Finding::corrupt("FONT_NO_TABLES", "numFonts 为 0", Some(8)));
        return;
    }
    let tbl_end = 12u64 + u64::from(num_fonts) * 4;
    if tbl_end > size {
        report.push(Finding::corrupt(
            "FONT_TABLE_DIRECTORY_TRUNCATED",
            format!("TTC 偏移表 {num_fonts} 项需要到 {tbl_end}，超出文件范围"),
            Some(8),
        ));
        return;
    }
    let mut buf = [0u8; 4];
    for i in 0..num_fonts {
        let at = 12u64 + u64::from(i) * 4;
        if r.seek(SeekFrom::Start(at)).is_err() || r.read_exact(&mut buf).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取 TTC 偏移表失败", Some(at)));
            return;
        }
        let off = u32::from_be_bytes(buf) as u64;
        if off + 12 > size {
            report.push(Finding::corrupt(
                "FONT_TTC_BAD_OFFSET",
                format!("字体 {i} 的 sfnt 头偏移 {off} 超出文件范围"),
                Some(at),
            ));
            return;
        }
        // 各字体只查结构边界，不重复做全量校验和
        verify_sfnt(r, off, size, size, &Opts::default(), report, false);
        if report
            .findings
            .iter()
            .any(|f| f.severity == crate::core::Severity::Corrupt)
        {
            return;
        }
    }
}
