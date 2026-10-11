use std::path::Path;

use super::{FileHint, Finding, Mode, Opts, Report, Severity, Verifier};
use crate::verifiers::{archive::ZipVerifier, generic, image::JpegVerifier, image::PngVerifier};

/// 魔数识别出的文件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicKind {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
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
    Sqlite,
    Parquet,
    Elf,
    Pe,
}

impl MagicKind {
    pub fn name(self) -> &'static str {
        match self {
            MagicKind::Png => "PNG",
            MagicKind::Jpeg => "JPEG",
            MagicKind::Gif => "GIF",
            MagicKind::Bmp => "BMP",
            MagicKind::Webp => "WebP",
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
            MagicKind::Sqlite => "SQLite",
            MagicKind::Parquet => "Parquet",
            MagicKind::Elf => "ELF",
            MagicKind::Pe => "PE",
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
    if starts(head, b"%PDF-") {
        return Some(MagicKind::Pdf);
    }
    if starts(head, b"{\\rtf") {
        return Some(MagicKind::Rtf);
    }
    if starts(head, b"\x1f\x8b") {
        return Some(MagicKind::Gzip);
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
    if starts(head, b"ID3") {
        return Some(MagicKind::Mp3);
    }
    if starts(head, &[0xFF, 0xE0, 0xFF]) || starts(head, &[0xFF, 0xFB]) {
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
        return Some(MagicKind::Mp4);
    }
    if head.len() >= 262 && &head[257..262] == b"ustar" {
        return Some(MagicKind::Tar);
    }
    None
}

/// 扩展名 → 期望的魔数类型
pub fn expected_by_ext(ext: &str) -> Option<MagicKind> {
    Some(match ext {
        "png" => MagicKind::Png,
        "jpg" | "jpeg" | "jpe" => MagicKind::Jpeg,
        "gif" => MagicKind::Gif,
        "bmp" | "dib" => MagicKind::Bmp,
        "webp" => MagicKind::Webp,
        "wav" | "wave" => MagicKind::Wav,
        "avi" => MagicKind::Avi,
        "gz" | "gzip" => MagicKind::Gzip,
        "zip" | "jar" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" => MagicKind::Zip,
        "7z" => MagicKind::SevenZ,
        "rar" => MagicKind::Rar,
        "pdf" => MagicKind::Pdf,
        "rtf" => MagicKind::Rtf,
        "flac" => MagicKind::Flac,
        "ogg" | "oga" | "ogv" => MagicKind::Ogg,
        "mp3" => MagicKind::Mp3,
        "mp4" | "m4a" | "m4v" | "mov" | "3gp" => MagicKind::Mp4,
        "sqlite" | "db" | "db3" => MagicKind::Sqlite,
        "parquet" => MagicKind::Parquet,
        _ => return None,
    })
}

fn registry() -> Vec<Box<dyn Verifier>> {
    vec![
        Box::new(PngVerifier),
        Box::new(JpegVerifier),
        Box::new(ZipVerifier),
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

    // 2. 按魔数（其次扩展名）选择格式验证器
    let reg = registry();
    if let Some(v) = reg.iter().find(|v| v.supports(&hint)) {
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
