use std::path::Path;

use super::{FileHint, Finding, Mode, Opts, Report, Severity, Verifier};
use crate::verifiers::{
    archive::{CompressVerifier, GzipVerifier, RarVerifier, SevenZVerifier, TarVerifier, ZipVerifier},
    binary::{ElfVerifier, MachoVerifier, PeVerifier},
    data::{CsvVerifier, ParquetVerifier, SqliteVerifier},
    doc::{DocContainerVerifier, PdfVerifier, RtfVerifier, TextDocVerifier},
    generic,
    image::{BmpVerifier, GifVerifier, IcoVerifier, JpegVerifier, PngVerifier, TiffVerifier, WebpVerifier},
    media::MediaVerifier,
    font::FontVerifier,
};

/// 魔数识别出的文件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicKind {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
    Tif,
    Wav,
    Avi,
    Gzip,
    Zip,
    SevenZ,
    Rar,
    Tar,
    Pdf,
    Rtf,
    Flac,
    Ogg,
    Mp3,
    Mp4,
    Mkv,
    Webm,
    Flv,
    Ts,
    Sqlite,
    Parquet,
    Elf,
    Pe,
    Heic,
    Avif,
    Ico,
    Font,
    Xz,
    Bzip2,
    Zstd,
    Lz4,
    Asf,
    Macho,
}

impl MagicKind {
    pub fn name(self) -> &'static str {
        match self {
            MagicKind::Png => "PNG",
            MagicKind::Jpeg => "JPEG",
            MagicKind::Gif => "GIF",
            MagicKind::Bmp => "BMP",
            MagicKind::Webp => "WebP",
            MagicKind::Tif => "TIFF",
            MagicKind::Wav => "WAV",
            MagicKind::Avi => "AVI",
            MagicKind::Gzip => "gzip",
            MagicKind::Zip => "ZIP",
            MagicKind::SevenZ => "7z",
            MagicKind::Rar => "RAR",
            MagicKind::Tar => "tar",
            MagicKind::Pdf => "PDF",
            MagicKind::Rtf => "RTF",
            MagicKind::Flac => "FLAC",
            MagicKind::Ogg => "OGG",
            MagicKind::Mp3 => "MP3",
            MagicKind::Mp4 => "MP4",
            MagicKind::Mkv => "Matroska",
            MagicKind::Webm => "WebM",
            MagicKind::Flv => "FLV",
            MagicKind::Ts => "MPEG-TS",
        MagicKind::Sqlite => "SQLite",
        MagicKind::Parquet => "Parquet",
        MagicKind::Elf => "ELF",
        MagicKind::Pe => "PE",
        MagicKind::Heic => "HEIC",
        MagicKind::Avif => "AVIF",
        MagicKind::Ico => "ICO",
        MagicKind::Font => "Font",
        MagicKind::Xz => "xz",
        MagicKind::Bzip2 => "bzip2",
        MagicKind::Zstd => "zstd",
        MagicKind::Lz4 => "LZ4",
        MagicKind::Asf => "ASF",
        MagicKind::Macho => "Mach-O",
    }
}
}

/// 依据头部字节识别真实类型；识别不出返回 None
pub fn detect_magic(head: &[u8]) -> Option<MagicKind> {
    fn starts(head: &[u8], sig: &[u8]) -> bool {
        head.len() >= sig.len() && &head[..sig.len()] == sig
    }

    if starts(head, b"\x89PNG\r\n\x1a\n") {
        return Some(MagicKind::Png);
    }
    if starts(head, &[0xFF, 0xD8, 0xFF]) {
        return Some(MagicKind::Jpeg);
    }
    if starts(head, b"GIF87a") || starts(head, b"GIF89a") {
        return Some(MagicKind::Gif);
    }
    if starts(head, b"II\x2a\x00")
        || starts(head, b"MM\x00\x2a")
        || starts(head, b"II\x2b\x00")
        || starts(head, b"MM\x00\x2b")
    {
        return Some(MagicKind::Tif);
    }
    if starts(head, b"%PDF-") {
        return Some(MagicKind::Pdf);
    }
    if starts(head, b"{\\rtf") {
        return Some(MagicKind::Rtf);
    }
    if starts(head, b"\x1f\x8b") {
        return Some(MagicKind::Gzip);
    }
    if starts(head, &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        return Some(MagicKind::Xz);
    }
    if starts(head, b"BZh") && head.len() >= 4 && (b'1'..=b'9').contains(&head[3]) {
        return Some(MagicKind::Bzip2);
    }
    if starts(head, &[0x28, 0xB5, 0x2F, 0xFD]) {
        return Some(MagicKind::Zstd);
    }
    if starts(head, &[0x04, 0x22, 0x4D, 0x18]) {
        return Some(MagicKind::Lz4);
    }
    if starts(
        head,
        &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA8, 0xB2, 0x00, 0xC0, 0x4F, 0xD9, 0x25, 0x22],
    ) {
        return Some(MagicKind::Asf);
    }
    if starts(head, &[0xFE, 0xED, 0xFA, 0xCE])
        || starts(head, &[0xFE, 0xED, 0xFA, 0xCF])
        || starts(head, &[0xCE, 0xFA, 0xED, 0xFE])
        || starts(head, &[0xCF, 0xFA, 0xED, 0xFE])
    {
        return Some(MagicKind::Macho);
    }
    if starts(head, b"PK\x03\x04") || starts(head, b"PK\x05\x06") || starts(head, b"PK\x07\x08") {
        return Some(MagicKind::Zip);
    }
    if starts(head, b"7z\xbc\xaf\x27\x1c") {
        return Some(MagicKind::SevenZ);
    }
    if starts(head, b"Rar!\x1a\x07") {
        return Some(MagicKind::Rar);
    }
    if starts(head, b"fLaC") {
        return Some(MagicKind::Flac);
    }
    if starts(head, b"OggS") {
        return Some(MagicKind::Ogg);
    }
    if starts(head, b"SQLite format 3\x00") {
        return Some(MagicKind::Sqlite);
    }
    if starts(head, b"PAR1") {
        return Some(MagicKind::Parquet);
    }
    if starts(head, b"\x7fELF") {
        return Some(MagicKind::Elf);
    }
    if starts(head, b"MZ") {
        return Some(MagicKind::Pe);
    }
    if starts(head, b"BM") && head.len() >= 14 {
        return Some(MagicKind::Bmp);
    }
    // ICO/CUR：ICONDIR reserved=0, type=1(ICO)/2(CUR)
    if head.len() >= 6
        && head[0] == 0
        && head[1] == 0
        && (head[2] == 1 || head[2] == 2)
        && head[3] == 0
    {
        return Some(MagicKind::Ico);
    }
    // 字体：sfnt / OpenType / 字体集合 / WOFF 系
    if starts(head, &[0x00, 0x01, 0x00, 0x00])
        || starts(head, b"OTTO")
        || starts(head, b"true")
        || starts(head, b"typ1")
        || starts(head, b"ttcf")
        || starts(head, b"wOFF")
        || starts(head, b"wOF2")
    {
        return Some(MagicKind::Font);
    }
    // UTF-16 BOM（FF FE / FE FF）与 MPEG 帧同步字节冲突，优先按文本处理
    if starts(head, &[0xFF, 0xFE]) || starts(head, &[0xFE, 0xFF]) {
        return None;
    }
    if starts(head, b"ID3") {
        // ID3v2 标签前缀：真实媒体签名在其后。常见于抓轨/下载站给 FLAC 贴标签，
        // 若直接判为 MP3 会把 FLAC 送进 MP3 验证器 → 假损坏（MP3_NO_FRAME）。
        return id3_payload_magic(head).or(Some(MagicKind::Mp3));
    }
    if is_mp3_sync(head) {
        return Some(MagicKind::Mp3);
    }
    if starts(head, b"RIFF") && head.len() >= 12 {
        return match &head[8..12] {
            b"WEBP" => Some(MagicKind::Webp),
            b"WAVE" => Some(MagicKind::Wav),
            b"AVI " => Some(MagicKind::Avi),
            _ => None,
        };
    }
    if head.len() >= 12
        && matches!(
            &head[4..8],
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide"
        )
    {
        if &head[4..8] == b"ftyp" {
            // major brand 决定 HEIC/AVIF，其余归 MP4
            let brand = &head[8..12];
            if matches!(
                brand,
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" | b"miaf" | b"heif"
            ) {
                return Some(MagicKind::Heic);
            }
            if matches!(brand, b"avif" | b"avis") {
                return Some(MagicKind::Avif);
            }
        }
        return Some(MagicKind::Mp4);
    }
    if starts(head, &[0x1A, 0x45, 0xDF, 0xA3]) {
        // EBML：按 DocType 区分 Matroska / WebM
        let webm = head.windows(4).any(|w| w == b"webm");
        return Some(if webm {
            MagicKind::Webm
        } else {
            MagicKind::Mkv
        });
    }
    if head.len() >= 9 && &head[0..3] == b"FLV" && head[3] == 1 {
        return Some(MagicKind::Flv);
    }
    if head.len() > 376 {
        // 至少三处同步字节，避免误判普通二进制/文本文件
        let std_ts = head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47;
        let m2ts = head[4] == 0x47 && head[192] == 0x47 && head[380] == 0x47;
        let fec_ts = head[0] == 0x47 && head[204] == 0x47;
        if std_ts || m2ts || fec_ts {
            return Some(MagicKind::Ts);
        }
    }
    if head.len() >= 262 && &head[257..262] == b"ustar" {
        return Some(MagicKind::Tar);
    }
    None
}

/// 判断字节流是否以合法 MPEG 音频帧同步字开头（完整校验，非仅 0xFF 0xEx）。
///
/// 校验位率/采样率索引是否为保留值——否则 ID3 标签内的 0xFF 0xFB 之类
/// 看似同步的字节会被误判成 MP3，抢在 fLaC 之前命中。
fn is_mp3_sync(b: &[u8]) -> bool {
    b.len() >= 4
        && b[0] == 0xFF
        && (b[1] & 0xE0) == 0xE0 // MPEG 同步
        && (b[1] & 0x06) != 0    // 层不是保留值
        && ((b[1] >> 3) & 0x03) != 1 // 版本不是保留值
        && (b[2] & 0xF0) != 0
        && (b[2] & 0xF0) != 0xF0 // 位率/采样率索引合法
        && (b[2] & 0x0C) != 0x0C
}

/// 跳过 ID3v2 标签后识别真实媒体签名（FLAC/Ogg/MP3 帧同步等）。
///
/// ID3v2 头 10 字节：`ID3` + 版本(2) + 标志(1) + synchsafe 长度(4)。
/// 总标签大小 = 10 + 长度；标志位 0x10 表示尾部还有 10 字节 footer。
/// 长度不可信/被截断时，回落到有界线性扫描（只找音频类签名，避免误命中其它魔数）。
fn id3_payload_magic(head: &[u8]) -> Option<MagicKind> {
    let mut start = id3_tag_len(head).unwrap_or(3); // 头损坏时从 ID3 之后线性扫
    if start >= head.len() {
        return None;
    }
    // 有界扫描：标签后通常紧跟签名，扫到 64 KiB（FileHint 只取前 512B，实际远小于）
    let end = head.len().min(start.saturating_add(65536));
    while start + 4 <= end {
        let w = &head[start..];
        if w.starts_with(b"fLaC") {
            return Some(MagicKind::Flac);
        }
        if w.starts_with(b"OggS") {
            return Some(MagicKind::Ogg);
        }
        if w.starts_with(b"ID3") {
            // 嵌套/连续标签：继续往后
            start += 3;
            continue;
        }
        // MP3 帧同步：与 detect_magic 主分支同样的完整校验（同步位/层/版本/位率/采样率）。
        // 只认 0xFF 0xFB 之类前缀会把 ID3 标签字节误判成 MP3，抢在 fLaC 之前返回。
        if is_mp3_sync(w) {
            return Some(MagicKind::Mp3);
        }
        if w.starts_with(b"RIFF") && w.len() >= 12 {
            return match &w[8..12] {
                b"WEBP" => Some(MagicKind::Webp),
                b"WAVE" => Some(MagicKind::Wav),
                b"AVI " => Some(MagicKind::Avi),
                _ => None,
            };
        }
        if w.len() >= 12
            && matches!(&w[4..8], b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide")
        {
            return Some(MagicKind::Mp4);
        }
        if w.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return Some(if head.windows(4).any(|x| x == b"webm") {
                MagicKind::Webm
            } else {
                MagicKind::Mkv
            });
        }
        start += 1;
    }
    None
}

/// 解析 ID3v2 标签总长度（含 10 字节头 + footer），失败返回 None。
/// 供 `FileHint::probe` 判断是否需扩大头部读取窗口。
pub fn id3_tag_len(head: &[u8]) -> Option<usize> {
    if head.len() < 10 || !head.starts_with(b"ID3") {
        return None;
    }
    // synchsafe：每字节低 7 位有效
    let b6 = head[6] as u32;
    let b7 = head[7] as u32;
    let b8 = head[8] as u32;
    let b9 = head[9] as u32;
    if b6 > 0x7F || b7 > 0x7F || b8 > 0x7F || b9 > 0x7F {
        return None; // 非法 synchsafe，长度不可信
    }
    let size = (b6 << 21) | (b7 << 14) | (b8 << 7) | b9;
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    Some((10 + size + footer) as usize)
}

/// 扩展名 → 期望的魔数类型
pub fn expected_by_ext(ext: &str) -> Option<MagicKind> {
    Some(match ext {
        "png" => MagicKind::Png,
        "jpg" | "jpeg" | "jpe" => MagicKind::Jpeg,
        "gif" => MagicKind::Gif,
        "bmp" | "dib" => MagicKind::Bmp,
        "webp" => MagicKind::Webp,
        "tif" | "tiff" => MagicKind::Tif,
        "wav" | "wave" => MagicKind::Wav,
        "avi" => MagicKind::Avi,
        "gz" | "gzip" | "tgz" => MagicKind::Gzip,
        "zip" | "jar" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" => MagicKind::Zip,
        "7z" => MagicKind::SevenZ,
        "rar" | "cbr" => MagicKind::Rar,
        "tar" => MagicKind::Tar,
        "pdf" => MagicKind::Pdf,
        "rtf" => MagicKind::Rtf,
        "flac" => MagicKind::Flac,
        "ogg" | "oga" | "ogv" => MagicKind::Ogg,
        "mp3" => MagicKind::Mp3,
        "mp4" | "m4a" | "m4v" | "mov" | "3gp" | "3g2" | "mp4v" => MagicKind::Mp4,
        "mkv" | "mka" | "mks" => MagicKind::Mkv,
        "webm" => MagicKind::Webm,
        "flv" | "f4v" => MagicKind::Flv,
        "sqlite" | "db" | "db3" => MagicKind::Sqlite,
        "parquet" => MagicKind::Parquet,
        "elf" | "o" | "so" | "ko" => MagicKind::Elf,
        "exe" | "dll" | "sys" | "ocx" | "cpl" | "scr" => MagicKind::Pe,
        "heic" | "heif" => MagicKind::Heic,
        "avif" => MagicKind::Avif,
        "ico" | "cur" => MagicKind::Ico,
        "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2" => MagicKind::Font,
        "xz" => MagicKind::Xz,
        "bz2" | "bz" => MagicKind::Bzip2,
        "zst" | "zstd" => MagicKind::Zstd,
        "lz4" => MagicKind::Lz4,
        "asf" | "wmv" | "wma" | "wax" | "wvx" => MagicKind::Asf,
        _ => return None,
    })
}

fn registry() -> Vec<Box<dyn Verifier>> {
    vec![
        Box::new(PngVerifier),
        Box::new(JpegVerifier),
        Box::new(GifVerifier),
        Box::new(BmpVerifier),
        Box::new(WebpVerifier),
        Box::new(TiffVerifier),
        // docx/xlsx/... 先做容器内容检查，ZIP 验证器再做 CRC
        Box::new(DocContainerVerifier),
        Box::new(ZipVerifier),
        Box::new(GzipVerifier),
        Box::new(TarVerifier),
        Box::new(SevenZVerifier),
        Box::new(RarVerifier),
        Box::new(PdfVerifier),
        Box::new(RtfVerifier),
        Box::new(SqliteVerifier),
        Box::new(CsvVerifier),
        Box::new(ParquetVerifier),
        Box::new(TextDocVerifier),
        Box::new(MediaVerifier),
        Box::new(ElfVerifier),
        Box::new(PeVerifier),
        Box::new(IcoVerifier),
        Box::new(FontVerifier),
        Box::new(CompressVerifier),
        Box::new(MachoVerifier),
    ]
}

/// 对单个文件执行盲检：通用检查 + 格式验证器 +（可选）sidecar 哈希比对
pub fn verify_file(path: &Path, opts: &Opts) -> Report {
    use std::time::Instant;

    let start = Instant::now();
    let hint = match FileHint::probe(path) {
        Ok(h) => h,
        Err(e) => {
            let mut report = Report::new(path.to_path_buf(), "unknown", Mode::Blind);
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("无法读取文件：{e}"),
                None,
            ));
            report.duration = start.elapsed();
            return report;
        }
    };

    let mut report = Report::new(path.to_path_buf(), hint.type_name(), Mode::Blind);

    // 1. 通用检查总是执行
    report.extend(generic::verify(path, &hint, opts).findings);

    // 2. 按魔数（其次扩展名）选择格式验证器；可叠加（如 docx 同时做容器与 ZIP 检查）
    let reg = registry();
    for v in reg.iter().filter(|v| v.supports(&hint)) {
        let sub = v.verify(path, opts);
        if hint.magic.is_none() && report.file_type == "unknown" {
            report.file_type = format!("{}（按扩展名推断）", sub.file_type);
        }
        report.extend(sub.findings);
    }

    // 3. 哈希清单比对（-m 指定的清单）
    if let Some(index) = &opts.manifest_index
        && let Some((kind, expected)) = index.get(path)
    {
        report.mode = Mode::Hash;
        report.extend(crate::core::hash::compare_hash(path, *kind, expected));
    }

    // 4. sidecar 哈希清单（存在则升级为 Hash 模式）
    if opts.sidecar_hash
        && let Some(sidecar) = crate::core::hash::find_sidecar(path)
    {
        report.mode = Mode::Hash;
        report.extend(crate::core::hash::verify_sidecar(path, &sidecar));
    }

    report.duration = start.elapsed();
    report
}

/// 供 UI/CLI 使用的最简结论统计
#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub total: usize,
    pub ok: usize,
    pub warning: usize,
    pub corrupt: usize,
    pub unknown: usize,
}

impl Summary {
    pub fn from_reports(reports: &[Report]) -> Self {
        let mut s = Self {
            total: reports.len(),
            ..Default::default()
        };
        for r in reports {
            match r.worst() {
                Severity::Ok => s.ok += 1,
                Severity::Warning => s.warning += 1,
                Severity::Corrupt => s.corrupt += 1,
                Severity::Unknown => s.unknown += 1,
            }
        }
        s
    }
}
