//! 音视频验证器：结构校验（纯 Rust）→ 深度解码（symphonia / ffmpeg）
//!
//! 覆盖 MP4/MOV、RIFF(WAV/AVI)、Matroska/WebM、FLV、MPEG-TS、MP3、FLAC、OGG。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Severity, Verifier};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Mp4,
    Wav,
    Avi,
    Mkv,
    Webm,
    Flv,
    Ts,
    Mp3,
    Flac,
    Ogg,
}

impl MediaKind {
    fn label(self) -> &'static str {
        match self {
            MediaKind::Mp4 => "MP4",
            MediaKind::Wav => "WAV",
            MediaKind::Avi => "AVI",
            MediaKind::Mkv => "Matroska",
            MediaKind::Webm => "WebM",
            MediaKind::Flv => "FLV",
            MediaKind::Ts => "MPEG-TS",
            MediaKind::Mp3 => "MP3",
            MediaKind::Flac => "FLAC",
            MediaKind::Ogg => "OGG",
        }
    }

    /// 容器格式（深度解码走 ffmpeg），音频流走 symphonia
    fn is_container(self) -> bool {
        matches!(
            self,
            MediaKind::Mp4
                | MediaKind::Avi
                | MediaKind::Mkv
                | MediaKind::Webm
                | MediaKind::Flv
                | MediaKind::Ts
        )
    }
}

fn head_looks_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return false;
    }
    if head.contains(&0) {
        return false;
    }
    let printable = head
        .iter()
        .filter(|&&b| b == b'\t' || b == b'\n' || b == b'\r' || (0x20..0x7f).contains(&b))
        .count();
    printable * 4 >= head.len() * 3
}

/// 决定该文件是否归本模块处理（魔数优先，扩展名兜底）
fn media_kind_of(hint: &FileHint) -> Option<MediaKind> {
    if let Some(k) = hint.magic {
        return match k {
            MagicKind::Mp4 => Some(MediaKind::Mp4),
            MagicKind::Wav => Some(MediaKind::Wav),
            MagicKind::Avi => Some(MediaKind::Avi),
            MagicKind::Mkv => Some(MediaKind::Mkv),
            MagicKind::Webm => Some(MediaKind::Webm),
            MagicKind::Flv => Some(MediaKind::Flv),
            MagicKind::Ts => Some(MediaKind::Ts),
            MagicKind::Mp3 => Some(MediaKind::Mp3),
            MagicKind::Flac => Some(MediaKind::Flac),
            MagicKind::Ogg => Some(MediaKind::Ogg),
            _ => None,
        };
    }

    // .ts 与 TypeScript 重名：内容像文本就不当视频处理
    if matches!(hint.ext.as_str(), "ts" | "mts" | "m2ts" | "m2t")
        && !head_looks_text(&hint.head)
        && hint.size >= 188
    {
        return Some(MediaKind::Ts);
    }

    let kind = match expected_by_ext(&hint.ext)? {
        MagicKind::Mp4 => MediaKind::Mp4,
        MagicKind::Wav => MediaKind::Wav,
        MagicKind::Avi => MediaKind::Avi,
        MagicKind::Mkv => MediaKind::Mkv,
        MagicKind::Webm => MediaKind::Webm,
        MagicKind::Flv => MediaKind::Flv,
        MagicKind::Ts => MediaKind::Ts,
        MagicKind::Mp3 => MediaKind::Mp3,
        MagicKind::Flac => MediaKind::Flac,
        MagicKind::Ogg => MediaKind::Ogg,
        _ => return None,
    };
    Some(kind)
}

pub struct MediaVerifier;

impl Verifier for MediaVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        media_kind_of(hint).is_some()
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let hint = match FileHint::probe(path) {
            Ok(h) => h,
            Err(e) => {
                let mut report = Report::new(path.to_path_buf(), "media", Mode::Blind);
                report.push(Finding::unknown(
                    "IO_ERROR",
                    format!("无法读取文件：{e}"),
                    None,
                ));
                return report;
            }
        };
        let Some(kind) = media_kind_of(&hint) else {
            return Report::new(path.to_path_buf(), "media", Mode::Blind);
        };

        let mut report = Report::new(path.to_path_buf(), kind.label(), Mode::Blind);
        let size = hint.size;
        if size == 0 {
            return report;
        }

        match kind {
            MediaKind::Mp4 => verify_mp4(path, size, &mut report),
            MediaKind::Wav | MediaKind::Avi => verify_riff(path, size, &mut report, kind),
            MediaKind::Mkv | MediaKind::Webm => verify_ebml(path, size, &mut report),
            MediaKind::Flv => verify_flv(path, size, &mut report),
            MediaKind::Ts => verify_ts(path, size, &mut report),
            MediaKind::Mp3 => verify_mp3(path, size, &mut report),
            MediaKind::Flac => verify_flac(path, size, opts, &mut report),
            MediaKind::Ogg => verify_ogg(path, size, &mut report),
        }

        if !has_corrupt(&report) {
            deep_decode(path, kind, opts, &mut report);
        }
        report
    }
}

fn has_corrupt(report: &Report) -> bool {
    report
        .findings
        .iter()
        .any(|f| f.severity == Severity::Corrupt)
}

fn io_error(report: &mut Report, e: std::io::Error) {
    report.push(Finding::unknown(
        "IO_ERROR",
        format!("读取文件失败：{e}"),
        None,
    ));
}

/// 顺序/随机读取工具：统一封装偏移读取
struct Stream {
    file: File,
    size: u64,
}

impl Stream {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self { file, size })
    }

    /// 从 `off` 起读满 `buf`，返回实际字节数（到 EOF 提前结束）
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> usize {
        if off >= self.size || buf.is_empty() {
            return 0;
        }
        if self.file.seek(SeekFrom::Start(off)).is_err() {
            return 0;
        }
        let want = buf.len().min((self.size - off) as usize);
        let mut total = 0;
        while total < want {
            match self.file.read(&mut buf[total..want]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(_) => break,
            }
        }
        total
    }

    fn take(&mut self, off: u64, n: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; n];
        (self.read_at(off, &mut buf) == n).then_some(buf)
    }

    fn u32(&mut self, off: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        (self.read_at(off, &mut b) == 4).then(|| u32::from_be_bytes(b))
    }

    fn u32le(&mut self, off: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        (self.read_at(off, &mut b) == 4).then(|| u32::from_le_bytes(b))
    }

    fn u64(&mut self, off: u64) -> Option<u64> {
        let mut b = [0u8; 8];
        (self.read_at(off, &mut b) == 8).then(|| u64::from_be_bytes(b))
    }
}

// ---------------------------------------------------------------------------
// MP4 / MOV / ISOBMFF
// ---------------------------------------------------------------------------

/// 顶层/子层 box 中属于「容器」的类型（内部还装着子 box）
const MP4_CONTAINERS: &[&[u8; 4]] = &[
    b"moov", b"trak", b"mdia", b"minf", b"stbl", b"udta", b"edts", b"mvex", b"dinf", b"moof",
    b"traf", b"mfra", b"skip",
];

#[derive(Default)]
struct Mp4Ctx {
    ftyp: bool,
    moov: bool,
    mdat: bool,
    /// 遇到致命结构问题后置位，所有层级立即停止
    stop: bool,
}

fn verify_mp4(path: &Path, size: u64, report: &mut Report) {
    let mut r = match Stream::open(path) {
        Ok(r) => r,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let mut ctx = Mp4Ctx::default();
    walk_mp4(&mut r, 0, size, size, 0, &mut ctx, report);
    if ctx.stop {
        return;
    }
    if !ctx.ftyp && !ctx.moov {
        report.push(Finding::corrupt(
            "MP4_BAD_HEADER",
            "既没有 ftyp 也没有 moov box，文件头已损坏",
            Some(0),
        ));
        return;
    }
    if !ctx.ftyp {
        report.push(Finding::warning(
            "MP4_NO_FTYP",
            "缺少 ftyp 类型标识 box",
            Some(0),
        ));
    }
    if !ctx.moov {
        report.push(Finding::corrupt(
            "MP4_NO_MOOV",
            "缺少 moov 影片元数据 box，文件可能被截断",
            Some(size),
        ));
    }
    if !ctx.mdat {
        report.push(Finding::warning(
            "MP4_NO_MDAT",
            "缺少 mdat 媒体数据 box",
            None,
        ));
    }
}

fn walk_mp4(
    r: &mut Stream,
    start: u64,
    end: u64,
    file_size: u64,
    depth: usize,
    ctx: &mut Mp4Ctx,
    report: &mut Report,
) {
    if ctx.stop || depth > 16 {
        return;
    }
    let mut off = start;
    while off + 8 <= end {
        let Some(size32) = r.u32(off) else { break };
        let Some(type_bytes) = r.take(off + 4, 4) else {
            break;
        };
        let typ: [u8; 4] = match type_bytes.as_slice().try_into() {
            Ok(t) => t,
            Err(_) => break,
        };

        // box 长度：0 = 一直延伸到父级末尾；1 = 后随 64 位长度
        let (payload_off, box_size) = if size32 == 0 {
            (off + 8, end - off)
        } else if size32 == 1 {
            match r.u64(off + 8) {
                Some(big) if big >= 16 => (off + 16, big),
                _ => {
                    report.push(Finding::corrupt(
                        "MP4_BAD_BOX",
                        "64 位扩展长度字段非法",
                        Some(off),
                    ));
                    ctx.stop = true;
                    return;
                }
            }
        } else {
            (off + 8, u64::from(size32))
        };

        if box_size < payload_off - off {
            report.push(Finding::corrupt(
                "MP4_BAD_BOX",
                format!(
                    "box {} 声明长度 {box_size} 小于头部长度",
                    String::from_utf8_lossy(&typ)
                ),
                Some(off),
            ));
            ctx.stop = true;
            return;
        }
        let box_end = off + box_size;
        if box_end > end {
            let name = String::from_utf8_lossy(&typ);
            report.push(Finding::corrupt(
                "MP4_TRUNCATED",
                if box_end > file_size {
                    format!("box {name} 声明长度 {box_size} 字节，超出文件范围（疑似截断）")
                } else {
                    format!("box {name} 声明长度 {box_size} 字节，超出父级 box 范围")
                },
                Some(off),
            ));
            ctx.stop = true;
            return;
        }
        if !typ.iter().all(|b| b.is_ascii_alphanumeric() || *b == b' ') {
            report.push(Finding::corrupt(
                "MP4_BAD_BOX",
                format!("box 类型字段非法（{typ:02x?}）"),
                Some(off + 4),
            ));
            ctx.stop = true;
            return;
        }

        match &typ {
            b"ftyp" => ctx.ftyp = true,
            b"moov" => ctx.moov = true,
            b"mdat" | b"moof" => ctx.mdat = true,
            _ => {}
        }

        if MP4_CONTAINERS.contains(&&typ) {
            walk_mp4(r, payload_off, box_end, file_size, depth + 1, ctx, report);
        } else if &typ == b"meta" {
            // meta 前面还有 4 字节 version/flags
            walk_mp4(
                r,
                (payload_off + 4).min(box_end),
                box_end,
                file_size,
                depth + 1,
                ctx,
                report,
            );
        } else {
            verify_mp4_leaf(r, &typ, payload_off, box_end, file_size, ctx, report);
        }
        if ctx.stop {
            return;
        }
        off = box_end;
    }

    if off < end && !ctx.stop {
        let remain = end - off;
        report.push(Finding::corrupt(
            if depth == 0 {
                "MP4_TRUNCATED"
            } else {
                "MP4_BAD_BOX"
            },
            format!("文件尾部剩余 {remain} 字节不足以构成一个完整的 box（疑似截断）"),
            Some(off),
        ));
        ctx.stop = true;
    }
}

/// 索引类叶子 box 的自洽校验（表长、偏移越界）
fn verify_mp4_leaf(
    r: &mut Stream,
    typ: &[u8; 4],
    payload: u64,
    end: u64,
    file_size: u64,
    ctx: &mut Mp4Ctx,
    report: &mut Report,
) {
    let len = end - payload;
    let name = String::from_utf8_lossy(typ);
    let mut bad = |report: &mut Report, msg: String| {
        report.push(Finding::corrupt("MP4_BAD_INDEX", msg, Some(payload)));
        ctx.stop = true;
    };

    match typ {
        b"stco" | b"co64" => {
            let entry = if typ == b"co64" { 8u64 } else { 4u64 };
            if len < 8 {
                bad(report, format!("{name} box 过短"));
                return;
            }
            let Some(count) = r.u32(payload + 4) else {
                bad(report, format!("{name} 缺少条目计数"));
                return;
            };
            if 8 + u64::from(count) * entry > len {
                bad(
                    report,
                    format!("{name} 记录 {count} 条偏移，超出 box 大小 {len} 字节"),
                );
                return;
            }
            let limit = count.min(4_000_000);
            for i in 0..limit {
                let at = payload + 8 + i as u64 * entry;
                let v = if typ == b"co64" {
                    r.u64(at)
                } else {
                    r.u32(at).map(u64::from)
                };
                let Some(v) = v else { break };
                if v >= file_size {
                    bad(
                        report,
                        format!("{name} 第 {} 项偏移 {v} 超出文件大小 {file_size}", i + 1),
                    );
                    return;
                }
            }
        }
        b"stsz" => {
            if len < 12 {
                bad(report, "stsz box 过短".to_string());
                return;
            }
            let (Some(sample_size), Some(count)) = (r.u32(payload + 4), r.u32(payload + 8)) else {
                bad(report, "stsz 字段读取失败".to_string());
                return;
            };
            if sample_size == 0 && 12 + u64::from(count) * 4 > len {
                bad(
                    report,
                    format!("stsz 记录 {count} 个样本尺寸，超出 box 大小 {len} 字节"),
                );
                return;
            }
            if count == 0 {
                report.push(Finding::warning(
                    "MP4_EMPTY_TABLE",
                    "stsz 样本数为 0",
                    Some(payload + 8),
                ));
            }
        }
        b"stsc" => {
            if len < 8 {
                bad(report, "stsc box 过短".to_string());
                return;
            }
            let Some(count) = r.u32(payload + 4) else {
                bad(report, "stsc 缺少条目计数".to_string());
                return;
            };
            if 8 + u64::from(count) * 12 > len {
                bad(
                    report,
                    format!("stsc 记录 {count} 条映射，超出 box 大小 {len} 字节"),
                );
                return;
            }
            let mut prev = 0u32;
            for i in 0..count.min(4_000_000) {
                let Some(first) = r.u32(payload + 8 + i as u64 * 12) else {
                    break;
                };
                if first < 1 || first <= prev {
                    bad(
                        report,
                        format!(
                            "stsc 第 {} 条 first_chunk={first}（上一条 {prev}），顺序非法",
                            i + 1
                        ),
                    );
                    return;
                }
                prev = first;
            }
        }
        b"stts" | b"ctts" => {
            if len < 8 {
                bad(report, format!("{name} box 过短"));
                return;
            }
            let Some(count) = r.u32(payload + 4) else {
                bad(report, format!("{name} 缺少条目计数"));
                return;
            };
            if 8 + u64::from(count) * 8 > len {
                bad(
                    report,
                    format!("{name} 记录 {count} 条，超出 box 大小 {len} 字节"),
                );
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// RIFF：WAV / AVI
// ---------------------------------------------------------------------------

fn verify_riff(path: &Path, size: u64, report: &mut Report, kind: MediaKind) {
    let mut r = match Stream::open(path) {
        Ok(r) => r,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let Some(head) = r.take(0, 12) else {
        report.push(Finding::corrupt(
            "RIFF_BAD_SIG",
            "文件不足 12 字节，缺少 RIFF 文件头",
            Some(0),
        ));
        return;
    };
    if &head[0..4] != b"RIFF" {
        report.push(Finding::corrupt(
            "RIFF_BAD_SIG",
            "缺少 RIFF 签名，文件头已损坏",
            Some(0),
        ));
        return;
    }
    let riff_size = u64::from(u32::from_le_bytes(head[4..8].try_into().unwrap()));
    let form: &[u8] = &head[8..12];
    let expect: &[u8] = if kind == MediaKind::Wav {
        b"WAVE"
    } else {
        b"AVI "
    };
    if form != expect {
        report.push(Finding::corrupt(
            "RIFF_BAD_FORM",
            format!(
                "RIFF 类型为 {}，期望 {}",
                String::from_utf8_lossy(form),
                String::from_utf8_lossy(expect)
            ),
            Some(8),
        ));
        return;
    }

    let declared = riff_size + 8;
    if declared > size {
        report.push(Finding::corrupt(
            "RIFF_TRUNCATED",
            format!("RIFF 头声明总长 {declared} 字节，实际只有 {size} 字节（疑似截断）"),
            Some(4),
        ));
        return;
    }
    if declared < size {
        report.push(Finding::warning(
            "RIFF_TRAILING_DATA",
            format!("RIFF 声明范围之外还有 {} 字节多余数据", size - declared),
            Some(declared),
        ));
    }
    let end = declared;

    let mut off = 12u64;
    let mut has_fmt = false;
    let mut has_data = false;
    let mut has_hdrl = false;
    let mut has_movi = false;
    let mut has_idx1 = false;
    let mut data_len = 0u64;

    while off + 8 <= end {
        let (Some(id), Some(len32)) = (r.take(off, 4), r.u32le(off + 4)) else {
            break;
        };
        if !id.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            report.push(Finding::corrupt(
                "RIFF_BAD_CHUNK",
                format!("偏移 {off} 处块标识非法（{id:02x?}），结构已损坏"),
                Some(off),
            ));
            return;
        }
        let len = u64::from(len32);
        let payload = off + 8;
        let chunk_end = payload + len;
        if chunk_end > size {
            report.push(Finding::corrupt(
                "RIFF_TRUNCATED",
                format!(
                    "块 {} 声明 {len32} 字节，超出文件范围（疑似截断）",
                    String::from_utf8_lossy(&id)
                ),
                Some(off),
            ));
            return;
        }
        if chunk_end > end {
            report.push(Finding::corrupt(
                "RIFF_BAD_CHUNK",
                format!(
                    "块 {} 超出 RIFF 声明范围",
                    String::from_utf8_lossy(&id)
                ),
                Some(off),
            ));
            return;
        }

        match &id[..] {
            b"fmt " => {
                has_fmt = true;
                verify_fmt_chunk(&mut r, payload, len, kind, report);
            }
            b"data" => {
                has_data = true;
                data_len = len;
            }
            b"LIST" => match r.take(payload, 4).as_deref() {
                Some(b"hdrl") => has_hdrl = true,
                Some(b"movi") => has_movi = true,
                _ => {}
            },
            b"idx1" => has_idx1 = true,
            _ => {}
        }

        let next = chunk_end + (len & 1); // 块按偶数字节对齐
        if next <= off || next > end {
            break;
        }
        off = next;
    }

    if kind == MediaKind::Wav {
        if !has_fmt {
            report.push(Finding::corrupt(
                "RIFF_NO_FMT",
                "WAV 缺少 fmt 格式描述块",
                Some(12),
            ));
        }
        if !has_data {
            report.push(Finding::corrupt(
                "RIFF_NO_DATA",
                "WAV 缺少 data 音频数据块",
                Some(12),
            ));
        } else if data_len == 0 {
            report.push(Finding::warning(
                "RIFF_EMPTY_DATA",
                "WAV 的 data 块长度为 0",
                Some(12),
            ));
        }
    } else {
        if !has_hdrl {
            report.push(Finding::corrupt(
                "RIFF_NO_HDRL",
                "AVI 缺少 hdrl 文件头列表，无法读取流信息",
                Some(12),
            ));
        }
        if !has_movi {
            report.push(Finding::corrupt(
                "RIFF_NO_MOVI",
                "AVI 缺少 movi 媒体数据列表，文件可能被截断",
                Some(end),
            ));
        }
        if !has_idx1 {
            report.push(Finding::warning(
                "RIFF_NO_IDX",
                "AVI 缺少 idx1 索引块（播放时需顺序扫描）",
                None,
            ));
        }
    }
}

/// 校验 fmt 块字段：WAV 按音频格式、AVI 按 BITMAPINFOHEADER
fn verify_fmt_chunk(
    r: &mut Stream,
    payload: u64,
    len: u64,
    kind: MediaKind,
    report: &mut Report,
) {
    if len < 16 {
        report.push(Finding::corrupt(
            "RIFF_BAD_FMT",
            format!("fmt 块只有 {len} 字节，不足 16 字节"),
            Some(payload),
        ));
        return;
    }
    let Some(f) = r.take(payload, len.min(64) as usize) else {
        report.push(Finding::corrupt(
            "RIFF_BAD_FMT",
            "fmt 块读取失败（疑似截断）",
            Some(payload),
        ));
        return;
    };

    if kind == MediaKind::Wav {
        let tag = u16::from_le_bytes([f[0], f[1]]);
        let channels = u16::from_le_bytes([f[2], f[3]]);
        let rate = u32::from_le_bytes(f[4..8].try_into().unwrap());
        let byte_rate = u32::from_le_bytes(f[8..12].try_into().unwrap());
        let block_align = u16::from_le_bytes([f[12], f[13]]);
        let bits = u16::from_le_bytes([f[14], f[15]]);
        if channels == 0 {
            report.push(Finding::corrupt(
                "RIFF_BAD_FMT",
                "WAV 声道数为 0",
                Some(payload + 2),
            ));
        }
        if rate == 0 {
            report.push(Finding::corrupt(
                "RIFF_BAD_FMT",
                "WAV 采样率为 0",
                Some(payload + 4),
            ));
        }
        if block_align == 0 {
            report.push(Finding::corrupt(
                "RIFF_BAD_FMT",
                "WAV block align 为 0",
                Some(payload + 12),
            ));
        }
        if !matches!(bits, 4 | 8 | 16 | 24 | 32 | 64) && tag != 0xFFFE {
            report.push(Finding::warning(
                "RIFF_BAD_FMT",
                format!("WAV 位深 {bits} 不常见"),
                Some(payload + 14),
            ));
        }
        if block_align != 0 && byte_rate != 0 && byte_rate != u32::from(block_align) * rate {
            report.push(Finding::warning(
                "RIFF_BAD_FMT",
                format!("WAV 字节率 {byte_rate} 与 声道x位深x采样率 不一致"),
                Some(payload + 8),
            ));
        }
    } else if len >= 40 {
        // BITMAPINFOHEADER
        let bi_size = u32::from_le_bytes(f[0..4].try_into().unwrap());
        let w = i32::from_le_bytes(f[4..8].try_into().unwrap());
        let h = i32::from_le_bytes(f[8..12].try_into().unwrap());
        if bi_size < 40 {
            report.push(Finding::warning(
                "RIFF_BAD_FMT",
                format!("AVI 头 biSize={bi_size} 小于 40"),
                Some(payload),
            ));
        }
        if w == 0 || h == 0 {
            report.push(Finding::corrupt(
                "RIFF_BAD_FMT",
                format!("AVI 视频尺寸非法：{w} x {h}"),
                Some(payload + 4),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// EBML：Matroska / WebM
// ---------------------------------------------------------------------------

/// 读 EBML 可变长整数：返回（占用字节数, 值, 是否为「未知大小」全 1）
fn read_vint(data: &[u8], at: usize) -> Option<(usize, u64, bool)> {
    let first = *data.get(at)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > 8 || at + width > data.len() {
        return None;
    }
    let mut value = u64::from(first) & (u64::from(0xFFu8) >> width);
    for i in 1..width {
        value = (value << 8) | u64::from(data[at + i]);
    }
    let all_ones = (1u64 << (7 * width)) - 1;
    Some((width, value, value == all_ones))
}

/// 读 EBML 元素 ID（含标识位的原始大端值）
fn read_id(data: &[u8], at: usize) -> Option<(usize, u32)> {
    let first = *data.get(at)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > 4 || at + width > data.len() {
        return None;
    }
    let mut value = 0u32;
    for i in 0..width {
        value = (value << 8) | u32::from(data[at + i]);
    }
    Some((width, value))
}

const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
const EBML_SEGMENT: u32 = 0x1853_8067;

/// 读取 `off` 处的元素头，返回（ID, 载荷起, 载荷止, 未知大小）
fn read_element_at(
    r: &mut Stream,
    off: u64,
    limit: u64,
) -> Option<(u32, u64, u64, bool)> {
    if off + 2 > limit {
        return None;
    }
    let win = r.take(off, 12.min((limit - off) as usize))?;
    let (w, id) = read_id(&win, 0)?;
    let (sw, sz, unknown) = read_vint(&win, w)?;
    let payload_start = off + (w + sw) as u64;
    let payload_end = if unknown {
        limit
    } else {
        payload_start.saturating_add(sz)
    };
    Some((id, payload_start, payload_end, unknown))
}

fn verify_ebml(path: &Path, size: u64, report: &mut Report) {
    let mut r = match Stream::open(path) {
        Ok(r) => r,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    if size < 5 {
        report.push(Finding::corrupt(
            "EBML_BAD_MAGIC",
            "文件不足 5 字节，缺少 EBML 签名",
            Some(0),
        ));
        return;
    }
    let Some(win) = r.take(0, 12) else {
        io_error(report, std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        return;
    };
    if win[..4] != EBML_MAGIC {
        report.push(Finding::corrupt(
            "EBML_BAD_MAGIC",
            "缺少 EBML 签名（1A 45 DF A3），文件头已损坏",
            Some(0),
        ));
        return;
    }

    // EBML 头部元素
    let Some((idw, _)) = read_id(&win, 0) else {
        report.push(Finding::corrupt(
            "EBML_BAD_MAGIC",
            "EBML 签名无法解析",
            Some(0),
        ));
        return;
    };
    let Some((sw, hdr_size, unknown)) = read_vint(&win, idw) else {
        report.push(Finding::corrupt(
            "EBML_BAD_VINT",
            "EBML 头长度字段非法",
            Some(idw as u64),
        ));
        return;
    };
    if unknown || hdr_size == 0 || hdr_size > 1024 * 1024 {
        report.push(Finding::corrupt(
            "EBML_BAD_VINT",
            format!("EBML 头长度 {hdr_size} 非法"),
            Some(idw as u64),
        ));
        return;
    }
    let header_payload = (idw + sw) as u64;
    let header_end = header_payload + hdr_size;
    if header_end > size {
        report.push(Finding::corrupt(
            "EBML_TRUNCATED",
            format!("EBML 头声明 {hdr_size} 字节，超出文件范围（疑似截断）"),
            Some(0),
        ));
        return;
    }

    // DocType 决定 matroska / webm
    let mut doctype = String::new();
    if let Some(hdr) = r.take(header_payload, hdr_size as usize) {
        let mut p = 0usize;
        while p + 2 < hdr.len() {
            let Some((w, id)) = read_id(&hdr, p) else {
                break;
            };
            let Some((sw2, sz2, unk2)) = read_vint(&hdr, p + w) else {
                break;
            };
            let start = p + w + sw2;
            let end = start + sz2 as usize;
            if unk2 || end > hdr.len() {
                break;
            }
            if id == 0x4282 {
                doctype = String::from_utf8_lossy(&hdr[start..end]).into_owned();
            }
            p = end;
        }
    }

    // 定位 Segment
    let mut off = header_end;
    let mut seg_end = None;
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        if off + 2 > size {
            break;
        }
        let Some((id, pstart, pend, unknown)) = read_element_at(&mut r, off, size) else {
            report.push(Finding::corrupt(
                "EBML_BAD_ELEMENT",
                format!("偏移 {off} 处元素头无法解析，结构已损坏"),
                Some(off),
            ));
            return;
        };
        if id == EBML_SEGMENT {
            if !unknown && pend > size {
                report.push(Finding::corrupt(
                    "EBML_TRUNCATED",
                    format!(
                        "Segment 声明结束于 {pend}，超出文件大小 {size}（疑似截断）"
                    ),
                    Some(off),
                ));
                return;
            }
            seg_end = Some(pend);
            off = pstart;
            break;
        }
        // Void / CRC-32 等顶层填充元素直接跳过
        if pend <= off || pend > size {
            report.push(Finding::corrupt(
                "EBML_BAD_ELEMENT",
                format!("偏移 {off} 处元素长度非法"),
                Some(off),
            ));
            return;
        }
        off = pend;
    }

    let Some(seg_end) = seg_end else {
        report.push(Finding::corrupt(
            "EBML_NO_SEGMENT",
            "未找到 Segment 元素，文件可能被截断",
            Some(header_end),
        ));
        return;
    };
    if seg_end < size {
        report.push(Finding::warning(
            "EBML_TRAILING_DATA",
            format!("Segment 之后还有 {} 字节多余数据", size - seg_end),
            Some(seg_end),
        ));
    }

    // 遍历 Segment 内的一级子元素
    let mut count = 0u32;
    while off < seg_end {
        if off + 2 > seg_end {
            report.push(Finding::corrupt(
                "EBML_TRUNCATED",
                format!("Segment 尾部剩余 {} 字节不足以构成一个元素", seg_end - off),
                Some(off),
            ));
            return;
        }
        let Some((_, _, pend, unknown)) = read_element_at(&mut r, off, seg_end) else {
            report.push(Finding::corrupt(
                "EBML_BAD_ELEMENT",
                format!("偏移 {off} 处元素头无法解析，结构已损坏"),
                Some(off),
            ));
            return;
        };
        if unknown {
            break; // 未知大小的末尾元素（如最后一个 Cluster）
        }
        if pend > seg_end || pend <= off {
            report.push(Finding::corrupt(
                "EBML_TRUNCATED",
                format!("偏移 {off} 处元素超出 Segment 范围（疑似截断）"),
                Some(off),
            ));
            return;
        }
        off = pend;
        count += 1;
        if count > 2_000_000 {
            break;
        }
    }

    if count == 0 {
        report.push(Finding::warning(
            "EBML_EMPTY",
            format!("Segment 中没有任何元素（DocType: {}）", doctype_or_unknown(&doctype)),
            Some(header_end),
        ));
    }
}

fn doctype_or_unknown(s: &str) -> &str {
    if s.is_empty() { "unknown" } else { s }
}

// ---------------------------------------------------------------------------
// MP3：ID3v2 + 帧链遍历
// ---------------------------------------------------------------------------

/// 位率表（kbps），索引 0 = free format，15 = 非法
const BR_MPEG1_L1: [u16; 16] = [
    0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
];
const BR_MPEG1_L2: [u16; 16] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
];
const BR_MPEG1_L3: [u16; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const BR_MPEG2_L1: [u16; 16] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
];
const BR_MPEG2_L23: [u16; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];
const SR_MPEG1: [u32; 3] = [44100, 48000, 32000];
const SR_MPEG2: [u32; 3] = [22050, 24000, 16000];
const SR_MPEG25: [u32; 3] = [11025, 12000, 8000];

#[derive(Clone, Copy)]
struct Mp3Frame {
    len: usize,
    rate: u32,
}

/// 解析 4 字节 MP3 帧头，返回帧长与采样率
fn parse_mp3_header(d: &[u8], i: usize) -> Option<Mp3Frame> {
    if i + 4 > d.len() {
        return None;
    }
    if d[i] != 0xFF || (d[i + 1] & 0xE0) != 0xE0 {
        return None;
    }
    // 版本：3 = MPEG1，2 = MPEG2，0 = MPEG2.5，1 = 保留
    let version = (d[i + 1] >> 3) & 0x03;
    if version == 1 {
        return None;
    }
    // 层：3 = LayerI，2 = LayerII，1 = LayerIII，0 = 保留
    let layer = (d[i + 1] >> 1) & 0x03;
    if layer == 0 {
        return None;
    }
    let bri = (d[i + 2] >> 4) as usize & 0x0F;
    let sri = (d[i + 2] >> 2) as usize & 0x03;
    if bri == 0 || bri == 15 || sri == 3 {
        return None;
    }
    let padding = u32::from((d[i + 2] >> 1) & 0x01);

    let kbps = u32::from(match (version, layer) {
        (3, 3) => BR_MPEG1_L1[bri],
        (3, 2) => BR_MPEG1_L2[bri],
        (3, 1) => BR_MPEG1_L3[bri],
        (_, 3) => BR_MPEG2_L1[bri],
        (_, _) => BR_MPEG2_L23[bri],
    });
    let base = match version {
        3 => SR_MPEG1,
        2 => SR_MPEG2,
        _ => SR_MPEG25,
    };
    let rate = base[sri];

    // 每帧样本数：LayerI=384；LayerII=1152；LayerIII=1152(MPEG1)/576(MPEG2,2.5)
    let samples = match layer {
        3 => 384,
        2 => 1152,
        _ if version == 3 => 1152,
        _ => 576,
    };
    let mut len = samples * kbps * 1000 / (8 * rate);
    len += if layer == 3 { padding * 4 } else { padding };
    if len < 4 {
        return None;
    }
    Some(Mp3Frame { len: len as usize, rate })
}

/// 尽量读满 `dst`，返回实际读到的字节数
fn read_fill(r: &mut impl Read, dst: &mut [u8]) -> usize {
    let mut total = 0;
    while total < dst.len() {
        match r.read(&mut dst[total..]) {
            Ok(0) => break,
            Ok(m) => total += m,
            Err(_) => break,
        }
    }
    total
}

/// 分块扫描：从 `from` 起找一个既合法又不越界的帧头（流式，不限文件大小）
fn find_mp3_frame(
    r: &mut BufReader<File>,
    from: u64,
    size: u64,
) -> Option<(u64, Mp3Frame)> {
    r.seek(SeekFrom::Start(from)).ok()?;
    let mut base = from;
    let mut buf = vec![0u8; 128 * 1024];
    let mut keep = 0usize;
    loop {
        // 尽量填满缓冲（保留 3 字节跨块候选）
        let mut n = 0usize;
        while keep + n < buf.len() {
            match r.read(&mut buf[keep + n..]) {
                Ok(0) => break,
                Ok(m) => n += m,
                Err(_) => break,
            }
        }
        let total = keep + n;
        if total >= 4 {
            let mut j = 0usize;
            while j + 4 <= total {
                if buf[j] == 0xFF
                    && let Some(f) = parse_mp3_header(&buf, j)
                {
                    let p = base + j as u64;
                    if p + f.len as u64 <= size {
                        return Some((p, f));
                    }
                }
                j += 1;
            }
        }
        if n == 0 {
            return None;
        }
        let tail = total.saturating_sub(3);
        buf.copy_within(tail..total, 0);
        keep = total - tail;
        base += tail as u64;
    }
}

fn verify_mp3(path: &Path, size: u64, report: &mut Report) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let mut r = BufReader::new(file);

    // ID3v2 标签
    let mut start = 0u64;
    let mut id3 = [0u8; 10];
    if read_fill(&mut r, &mut id3) >= 10 && &id3[0..3] == b"ID3" {
        if id3[6] & 0x80 != 0
            || id3[7] & 0x80 != 0
            || id3[8] & 0x80 != 0
            || id3[9] & 0x80 != 0
        {
            report.push(Finding::corrupt(
                "MP3_BAD_ID3",
                "ID3v2 长度字段不是合法的同步安全编码",
                Some(6),
            ));
            return;
        }
        let tag_size = (u32::from(id3[6]) << 21)
            | (u32::from(id3[7]) << 14)
            | (u32::from(id3[8]) << 7)
            | u32::from(id3[9]);
        let footer = if id3[5] & 0x10 != 0 { 10u64 } else { 0 };
        let tag_end = 10u64.saturating_add(u64::from(tag_size)).saturating_add(footer);
        if tag_end > size {
            report.push(Finding::corrupt(
                "MP3_ID3_TRUNCATED",
                format!("ID3v2 标签声明 {tag_size} 字节，超出文件范围（疑似截断）"),
                Some(0),
            ));
            return;
        }
        start = tag_end;
    }

    let Some((first, _)) = find_mp3_frame(&mut r, start, size) else {
        report.push(Finding::corrupt(
            "MP3_NO_FRAME",
            "未找到任何合法的 MP3 帧头（文件损坏或格式不符）",
            Some(start),
        ));
        return;
    };
    if first > start {
        let gap = first - start;
        if gap > 4096 {
            report.push(Finding::corrupt(
                "MP3_GARBAGE",
                format!("首帧之前有 {gap} 字节无法解析的数据"),
                Some(start),
            ));
            return;
        }
        report.push(Finding::warning(
            "MP3_LEADING_DATA",
            format!("首帧之前有 {gap} 字节非音频数据"),
            Some(start),
        ));
    }

    let mut pos = first;
    let mut prev_rate = 0u32;
    let mut frames: u64 = 0;
    let mut inconsistent = false;

    r.seek(SeekFrom::Start(pos)).ok();
    loop {
        let mut h = [0u8; 4];
        let frame = (read_fill(&mut r, &mut h) == 4).then(|| parse_mp3_header(&h, 0)).flatten();
        let Some(frame) = frame else {
            report.push(Finding::corrupt(
                "MP3_BAD_FRAME",
                format!("偏移 {pos} 处帧头无法解析，帧链已断裂"),
                Some(pos),
            ));
            return;
        };
        if !inconsistent && prev_rate != 0 && prev_rate != frame.rate {
            inconsistent = true;
            report.push(Finding::warning(
                "MP3_INCONSISTENT",
                format!(
                    "第 {} 帧采样率 {} Hz，与前帧 {prev_rate} Hz 不一致",
                    frames + 1,
                    frame.rate
                ),
                Some(pos),
            ));
        }
        prev_rate = frame.rate;

        let end = pos + frame.len as u64;
        if end > size {
            report.push(Finding::corrupt(
                "MP3_TRUNCATED",
                format!(
                    "第 {} 帧声明 {} 字节，超出文件末尾 {end} > {size}（疑似截断）",
                    frames + 1,
                    frame.len
                ),
                Some(pos),
            ));
            return;
        }
        frames += 1;
        if frames > 5_000_000 {
            report.push(Finding::warning(
                "MP3_TOO_MANY_FRAMES",
                "帧数超过 500 万，提前结束遍历",
                None,
            ));
            return;
        }
        if end == size {
            break; // 正常走到文件尾
        }

        // 尾部标签：读至多 8 字节窥探，随后绝对回退
        let rest_len = size - end;
        let mut peek = [0u8; 8];
        let pn = read_fill(&mut r, &mut peek);
        if pn >= 3 && &peek[..3] == b"TAG" {
            if rest_len != 128 {
                report.push(Finding::warning(
                    "MP3_BAD_TAIL",
                    format!("ID3v1 标签实际 {rest_len} 字节，应为 128 字节"),
                    Some(end),
                ));
            }
            break;
        }
        if pn >= 8 && &peek[..8] == b"APETAGEX" {
            break;
        }

        r.seek(SeekFrom::Start(end)).ok();
        // 尝试直接续帧
        let mut h2 = [0u8; 4];
        let cont = read_fill(&mut r, &mut h2) == 4 && parse_mp3_header(&h2, 0).is_some();
        if cont {
            r.seek(SeekFrom::Start(end)).ok();
            pos = end;
            continue;
        }

        // 帧链中断：向前搜索下一个合法帧头，其间的字节即损坏区间
        match find_mp3_frame(&mut r, end + 1, size) {
            Some((q, _)) => {
                report.push(Finding::corrupt(
                    "MP3_GARBAGE",
                    format!(
                        "帧链在 {end} 处中断，其后 {} 字节无法解析（数据区被破坏），下一合法帧在 {q}",
                        q - end
                    ),
                    Some(end),
                ));
            }
            None => {
                report.push(Finding::corrupt(
                    "MP3_BAD_FRAME",
                    format!("帧链在 {end} 处中断，其后没有可识别的帧头"),
                    Some(end),
                ));
            }
        }
        return;
    }

    if frames == 0 {
        report.push(Finding::corrupt(
            "MP3_NO_FRAME",
            "文件中没有完整的 MP3 帧",
            Some(start),
        ));
    }
}

// ---------------------------------------------------------------------------
// FLAC：元数据链 + 帧 CRC8/CRC16
// ---------------------------------------------------------------------------

/// 解析帧头（含 CRC8 校验），返回含 CRC8 在内的帧头长度
fn flac_frame_header_len(d: &[u8], i: usize) -> Option<usize> {
    if i + 6 > d.len() {
        return None;
    }
    if d[i] != 0xFF || (d[i + 1] & 0xFC) != 0xF8 {
        return None;
    }
    if d[i + 2] & 0xF0 == 0 {
        return None; // 块大小保留值
    }
    if d[i + 2] & 0x0F == 0x0F {
        return None; // 采样率保留值
    }
    if (d[i + 3] >> 4) >= 0x0B {
        return None; // 声道分配保留值
    }
    if d[i + 3] & 0x0E == 0x06 || d[i + 3] & 0x0E == 0x0E {
        return None; // 位深保留值
    }
    if d[i + 3] & 0x01 == 1 {
        return None; // 保留位必须为 0
    }

    let mut p = i + 4;
    // UTF-8 编码的帧号/样本号
    let b0 = *d.get(p)?;
    let extra = match b0 {
        0x00..=0x7F => 0,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        0xF8..=0xFB => 4,
        0xFC..=0xFD => 5,
        0xFE => 6,
        _ => return None,
    };
    p += 1;
    for _ in 0..extra {
        let b = *d.get(p)?;
        if b & 0xC0 != 0x80 {
            return None;
        }
        p += 1;
    }

    // 可选的块大小 / 采样率补充字节
    let bs_code = d[i + 2] >> 4;
    if bs_code == 6 {
        p += 1;
    } else if bs_code == 7 {
        p += 2;
    }
    let sr_code = d[i + 2] & 0x0F;
    if sr_code == 0x0C {
        p += 1;
    } else if sr_code == 0x0D || sr_code == 0x0E {
        p += 2;
    }

    let crc_at = *d.get(p)?;
    if crc8_flac(&d[i..p]) != crc_at {
        return None;
    }
    Some(p + 1 - i)
}

fn crc8_flac(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// 从 `from` 起找合法帧头，返回（起始位置, 帧头长度）
fn find_flac_header(d: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 6 <= d.len() {
        if d[i] == 0xFF
            && let Some(hlen) = flac_frame_header_len(d, i)
        {
            return Some((i, hlen));
        }
        i += 1;
    }
    None
}

fn crc16_update(crc: u16, b: u8) -> u16 {
    let mut crc = crc ^ (u16::from(b) << 8);
    for _ in 0..8 {
        crc = if crc & 0x8000 != 0 {
            (crc << 1) ^ 0x8005
        } else {
            crc << 1
        };
    }
    crc
}

fn is_tag_tail(d: &[u8], i: usize) -> bool {
    let rest = &d[i..];
    (rest.len() >= 3 && &rest[..3] == b"TAG") || (rest.len() >= 8 && &rest[..8] == b"APETAGEX")
}

/// 搜索帧尾：返回帧结束位置的下一字节（即下一帧起点）
fn find_flac_frame_end(d: &[u8], start: usize, hlen: usize) -> Option<usize> {
    let mut crc = 0u16;
    let mut e = start + 1;
    while e + 2 <= d.len() {
        crc = crc16_update(crc, d[e - 1]);
        if e >= start + hlen && crc == u16::from_be_bytes([d[e], d[e + 1]]) {
            let next = e + 2;
            if next == d.len() || flac_frame_header_len(d, next).is_some() || is_tag_tail(d, next)
            {
                return Some(next);
            }
        }
        e += 1;
    }
    None
}

/// 结构遍历的大小上限（整读进内存做逐帧 CRC 检查，超过则不整读）；
/// 超过的文件在勾选「强制深度解码」时改走流式全量 MD5（不占内存）。
const FLAC_STRUCTURAL_LIMIT: u64 = 512 * 1024 * 1024;

fn verify_flac(path: &Path, size: u64, opts: &Opts, report: &mut Report) {
    if size > FLAC_STRUCTURAL_LIMIT {
        if opts.deep {
            // 强制深度解码：GB 级整轨不适合整读进内存做结构遍历，但流式全量
            // 解码 MD5 是金标准且内存恒定——只读头部拿 STREAMINFO 期望值即可。
            let stored = read_head_streaminfo_md5(path);
            verify_flac_md5(path, size, opts, stored, report);
        } else {
            // 未执行不等于有问题（对齐 BATON「未启用的检查不算问题」）：
            // 降为 Ok 级说明，详情仍可见；勾选「强制深度解码」可做全量 MD5。
            report.push(Finding::ok(
                "FLAC_TOO_LARGE",
                format!(
                    "文件 {size} 字节超过 {} MiB，跳过结构遍历与 MD5 校验（可在设置勾选「强制深度解码」做全量 MD5）",
                    FLAC_STRUCTURAL_LIMIT / (1024 * 1024)
                ),
            ));
        }
        return;
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    if data.len() < 4 || &data[0..4] != b"fLaC" {
        report.push(Finding::corrupt(
            "FLAC_BAD_MAGIC",
            "缺少 fLaC 签名，文件头已损坏",
            Some(0),
        ));
        return;
    }

    // 元数据块链
    let mut p = 4usize;
    let mut last = false;
    let mut blocks = 0u32;
    let mut has_streaminfo = false;
    let mut stored_md5: Option<[u8; 16]> = None;
    while !last {
        if p + 4 > data.len() {
            report.push(Finding::corrupt(
                "FLAC_TRUNCATED",
                format!("偏移 {p} 处元数据块头不完整（疑似截断）"),
                Some(p as u64),
            ));
            return;
        }
        let flags = data[p];
        last = flags & 0x80 != 0;
        let btype = flags & 0x7F;
        let len = (usize::from(data[p + 1]) << 16)
            | (usize::from(data[p + 2]) << 8)
            | usize::from(data[p + 3]);
        if p + 4 + len > data.len() {
            report.push(Finding::corrupt(
                "FLAC_TRUNCATED",
                format!("元数据块 {btype} 声明 {len} 字节，超出文件范围（疑似截断）"),
                Some(p as u64),
            ));
            return;
        }
        if blocks == 0 && btype != 0 {
            report.push(Finding::corrupt(
                "FLAC_NO_STREAMINFO",
                "第一个元数据块不是 STREAMINFO，文件头已损坏",
                Some(p as u64),
            ));
            return;
        }
        if btype == 0 {
            if len != 34 {
                report.push(Finding::corrupt(
                    "FLAC_BAD_STREAMINFO",
                    format!("STREAMINFO 长度应为 34，实际 {len}"),
                    Some(p as u64),
                ));
                return;
            }
            has_streaminfo = true;
            check_streaminfo(&data[p + 4..p + 4 + len], report, p as u64 + 4);
            stored_md5 = streaminfo_md5(&data[p + 4..p + 4 + len]);
        }
        p += 4 + len;
        blocks += 1;
        if blocks > 100_000 {
            report.push(Finding::corrupt(
                "FLAC_BAD_METADATA",
                "元数据块数量异常（结构已损坏）",
                Some(4),
            ));
            return;
        }
    }

    if !has_streaminfo {
        report.push(Finding::corrupt(
            "FLAC_NO_STREAMINFO",
            "缺少 STREAMINFO 块，无法读取音频流信息",
            Some(4),
        ));
        return;
    }

    // 音频帧区
    if p >= data.len() {
        report.push(Finding::corrupt(
            "FLAC_NO_FRAME",
            "元数据之后没有任何音频帧（文件可能被截断）",
            Some(size),
        ));
        return;
    }

    let mut pos = p;
    let mut frames: u64 = 0;
    loop {
        let Some(hlen) = flac_frame_header_len(&data, pos) else {
            match find_flac_header(&data, pos) {
                Some((i, _)) if i > pos => {
                    report.push(Finding::corrupt(
                        "FLAC_FRAME_GAP",
                        format!(
                            "偏移 {pos} 处有 {} 字节无法解析（帧间存在垃圾数据），下一帧在 {i}",
                            i - pos
                        ),
                        Some(pos as u64),
                    ));
                }
                _ => {
                    report.push(Finding::corrupt(
                        "FLAC_BAD_FRAME",
                        format!("偏移 {pos} 处无法解析出合法的音频帧头，文件可能被截断"),
                        Some(pos as u64),
                    ));
                }
            }
            return;
        };

        let Some(next) = find_flac_frame_end(&data, pos, hlen) else {
            report.push(Finding::corrupt(
                "FLAC_TRUNCATED",
                format!("偏移 {pos} 处的音频帧没有合法的帧尾（CRC16 校验失败，疑似截断）"),
                Some(pos as u64),
            ));
            return;
        };

        frames += 1;
        if frames > 10_000_000 {
            report.push(Finding::warning(
                "FLAC_TOO_MANY_FRAMES",
                "帧数超过 1000 万，提前结束遍历",
                None,
            ));
            return;
        }
        if next >= data.len() {
            break;
        }
        pos = next;
    }

    if frames == 0 {
        report.push(Finding::corrupt(
            "FLAC_NO_FRAME",
            "文件中没有完整的音频帧",
            Some(p as u64),
        ));
    }

    // 结构层通过后，做金标准校验：完整解码并比对 STREAMINFO 里的音频 MD5。
    // 这一步等价于 `flac -t`，能发现结构校验看不见的音频数据损坏。
    if !has_corrupt(report) {
        verify_flac_md5(path, size, opts, stored_md5, report);
    }
}

/// FLAC 金标准校验：完整解码整段音频，算 PCM 的 MD5 并与 STREAMINFO 比对。
///
/// 这是 FlacValidator / `flac -t` 使用的方法。结构层（CRC8/CRC16）只能证明
/// "帧能被正确切分"，MD5 才能证明"解码出的音频一个样本都没变"。
fn verify_flac_md5(path: &Path, size: u64, opts: &Opts, stored: Option<[u8; 16]>, report: &mut Report) {
    let Some(stored) = stored else {
        // 未记录 MD5 是编码器/年代问题，不是文件损坏；结构校验已通过，故为 Ok 级提示。
        report.push(Finding::ok(
            "FLAC_NO_MD5",
            "STREAMINFO 未记录音频 MD5（多为老 rip），无法做完整校验（结构校验已通过）",
        ));
        return;
    };

    // 不受深度解码大小阈值限制：结构校验已通过，完整 MD5 是金标准，对齐
    // FlacValidator 的全量解码口径（整轨 FLAC 也应享受）。只有 512 MiB 安全
    // 上限；超过上限只是「没做完整 MD5 校验」，不是文件有问题，故为 Ok 级说明。
    // 解码是流式的（内存与文件大小无关），用户勾选「强制深度解码」(-d) 可忽略上限。
    let computed = match decode_flac_md5(path, opts.deep) {
        Ok(computed) => computed,
        Err(FlacDecodeError::TooLarge) => {
            report.push(Finding::ok(
                "FLAC_MD5_TOO_LARGE",
                format!(
                    "文件 {size} 字节超过硬上限 {} MiB，未做完整 MD5 校验（结构校验已通过；勾选「强制深度解码」可忽略上限）",
                    FLAC_MD5_HARD_LIMIT / (1024 * 1024)
                ),
            ));
            return;
        }
        Err(FlacDecodeError::DecodeFailed) => {
            report.push(Finding::corrupt(
                "FLAC_MD5_DECODE_FAILED",
                "FLAC 深度解码中途失败（音频帧损坏），MD5 无法完成校验",
                None,
            ));
            return;
        }
        Err(FlacDecodeError::Unavailable) => {
            report.push(Finding::ok(
                "FLAC_MD5_SKIPPED",
                "FLAC 深度解码未执行（解码器不可用），未做完整 MD5 校验（结构校验已通过）",
            ));
            return;
        }
    };

    if computed == stored {
        report.push(Finding::ok(
            "FLAC_MD5_OK",
            "完整解码通过，音频 MD5 与 STREAMINFO 一致（数据完好）",
        ));
    } else {
        report.push(Finding::corrupt(
            "FLAC_MD5_MISMATCH",
            format!(
                "音频 MD5 不匹配：期望 {stored:x}，实际 {computed:x}（音频数据已损坏）",
                stored = Hex(&stored),
                computed = Hex(&computed),
            ),
            None,
        ));
    }
}

struct Hex<'a>(&'a [u8]);

impl std::fmt::LowerHex for Hex<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// 完整解码 FLAC，并按 RFC 9639 §8.2 计算音频 MD5。
///
/// RFC 9639 规定 MD5 的输入是"所有声道、按样本交错、有符号小端表示"的字节流；
/// 位深不是整数字节时样本符号扩展到下一个整字节数。这里对齐 libFLAC 的
/// `format_input_`，与 `flac -t` / FlacValidator 的比对口径完全一致。
/// 深度解码 MD5 的默认硬上限：超过该大小的文件不做完整 MD5（Ok 级说明），
/// 防止批量扫描被极端大文件拖死；勾选「强制深度解码」(`-d`) 时忽略此上限。
const FLAC_MD5_HARD_LIMIT: u64 = 512 * 1024 * 1024;

enum FlacDecodeError {
    /// 文件超过硬上限
    TooLarge,
    /// 打开/探测/建解码器任一步失败
    Unavailable,
    /// 解码中途失败（帧损坏等）
    DecodeFailed,
}

fn decode_flac_md5(path: &Path, force: bool) -> Result<[u8; 16], FlacDecodeError> {
    use sha2::Digest as _;
    use symphonia::core::audio::Signal;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    if !force
        && let Ok(meta) = std::fs::metadata(path)
        && meta.len() > FLAC_MD5_HARD_LIMIT
    {
        return Err(FlacDecodeError::TooLarge);
    }
    let file = File::open(path).map_err(|_| FlacDecodeError::Unavailable)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| FlacDecodeError::Unavailable)?;
    let mut format = probed.format;
    let Some(track) = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
    else {
        return Err(FlacDecodeError::Unavailable);
    };
    let track_id = track.id;
    // 原生位深取自 codec 参数；缺失时回退 16（CD），保证字节宽度确定。
    // symphonia 的 FLAC 解码器把样本左移填满 32 位（见 decoder.rs 的
    // `sample << (32 - bits_per_sample)`），所以这里必须再右移回原生位深，
    // 否则截低 N 字节会得到全零。
    let bits = track.codec_params.bits_per_sample.unwrap_or(16).clamp(8, 32);
    let shift = 32 - bits;
    let bytes_per_sample = (bits as usize).div_ceil(8).clamp(1, 4);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| FlacDecodeError::Unavailable)?;

    let mut hasher = md5::Md5::new();
    let mut inter: Vec<u8> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(a) => a,
            Err(_) => return Err(FlacDecodeError::DecodeFailed),
        };
        // 转成 planar i32，右移回原生位深，再交错取低 N 字节（RFC 9639 §8.2）
        let mut scratch = audio.make_equivalent::<i32>();
        audio.convert(&mut scratch);
        let ch = scratch.spec().channels.count();
        let frames = scratch.frames();
        inter.clear();
        inter.reserve(frames * ch * bytes_per_sample);
        for f in 0..frames {
            for c in 0..ch {
                let s = (scratch.chan(c)[f] >> shift) as u32;
                // 小端、只取低 bytes_per_sample 字节（等价于符号扩展后截断）
                inter.extend_from_slice(&s.to_le_bytes()[..bytes_per_sample]);
            }
        }
        hasher.update(&inter);
    }
    Ok(hasher.finalize().into())
}

/// 解析 STREAMINFO 中存储的音频 MD5（16 字节大端）。
///
/// FLAC 规范：STREAMINFO 偏移 18..34 存整段解码 PCM 的 MD5。
/// 全零表示编码时未记录校验值（老 rip 常见），返回 None。
fn streaminfo_md5(si: &[u8]) -> Option<[u8; 16]> {
    if si.len() < 34 {
        return None;
    }
    let md5: [u8; 16] = si[18..34].try_into().ok()?;
    if md5.iter().all(|&b| b == 0) {
        return None; // 未记录校验值
    }
    Some(md5)
}

/// 大文件强制深度解码时只读文件头取 STREAMINFO 的期望 MD5（避免为 16 字节
/// 把 GB 级文件读进内存）。STREAMINFO 固定为第一个元数据块（type 0、长 34），
/// 开头 42 字节内即可拿到。
fn read_head_streaminfo_md5(path: &Path) -> Option<[u8; 16]> {
    use std::io::Read as _;

    let mut f = File::open(path).ok()?;
    let mut buf = [0u8; 4 + 4 + 34];
    f.read_exact(&mut buf).ok()?;
    if &buf[0..4] != b"fLaC" || buf[4] & 0x7F != 0 {
        return None;
    }
    let len = (usize::from(buf[5]) << 16) | (usize::from(buf[6]) << 8) | usize::from(buf[7]);
    if len < 34 {
        return None;
    }
    streaminfo_md5(&buf[8..])
}

fn check_streaminfo(si: &[u8], report: &mut Report, offset: u64) {
    if si.len() < 34 {
        return;
    }
    let min_bs = u16::from_be_bytes([si[0], si[1]]);
    let max_bs = u16::from_be_bytes([si[2], si[3]]);
    let mut packed = 0u64;
    for &b in &si[10..18] {
        packed = (packed << 8) | u64::from(b);
    }
    let rate = (packed >> 44) as u32;
    let channels = ((packed >> 41) & 0x7) as u32 + 1;
    let bps = ((packed >> 36) & 0x1F) as u32 + 1;

    if rate == 0 {
        report.push(Finding::corrupt(
            "FLAC_BAD_STREAMINFO",
            "STREAMINFO 采样率为 0",
            Some(offset + 10),
        ));
    }
    if channels > 8 {
        report.push(Finding::corrupt(
            "FLAC_BAD_STREAMINFO",
            format!("STREAMINFO 声道数非法：{channels}"),
            Some(offset + 10),
        ));
    }
    if !(4..=32).contains(&bps) {
        report.push(Finding::corrupt(
            "FLAC_BAD_STREAMINFO",
            format!("STREAMINFO 位深非法：{bps}"),
            Some(offset + 10),
        ));
    }
    if min_bs > max_bs {
        report.push(Finding::corrupt(
            "FLAC_BAD_STREAMINFO",
            format!("STREAMINFO 最小块 {min_bs} 大于最大块 {max_bs}"),
            Some(offset),
        ));
    } else if min_bs != 0 && min_bs < 16 {
        report.push(Finding::warning(
            "FLAC_BAD_STREAMINFO",
            format!("STREAMINFO 最小块 {min_bs} 小于规范要求的 16"),
            Some(offset),
        ));
    }
}

// ---------------------------------------------------------------------------
// OGG：页 CRC + 逻辑流完整性
// ---------------------------------------------------------------------------

fn ogg_crc_table() -> &'static [u32; 256] {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 {
                    (r << 1) ^ 0x04C1_1DB7
                } else {
                    r << 1
                };
            }
            *slot = r;
        }
        table
    })
}

fn ogg_crc(data: &[u8]) -> u32 {
    let table = ogg_crc_table();
    let mut crc = 0u32;
    for &b in data {
        crc = (crc << 8) ^ table[(((crc >> 24) ^ u32::from(b)) & 0xFF) as usize];
    }
    crc
}

fn verify_ogg(path: &Path, size: u64, report: &mut Report) {
    let mut r = match Stream::open(path) {
        Ok(r) => r,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let mut off = 0u64;
    let mut pages = 0u64;
    let mut saw_eos = false;

    while off < size {
        let header = match r.take(off, 27.min((size - off) as usize)) {
            Some(h) => h,
            None => {
                report.push(Finding::corrupt(
                    "OGG_TRUNCATED",
                    format!("偏移 {off} 处页头不完整（疑似截断）"),
                    Some(off),
                ));
                return;
            }
        };
        if header.len() < 27 || &header[0..4] != b"OggS" {
            report.push(Finding::corrupt(
                "OGG_BAD_PAGE",
                format!("偏移 {off} 处缺少 OggS 页签名，结构已损坏"),
                Some(off),
            ));
            return;
        }
        if header[4] != 0 {
            report.push(Finding::corrupt(
                "OGG_BAD_PAGE",
                format!("偏移 {off} 处页版本号 {} 非法", header[4]),
                Some(off + 4),
            ));
            return;
        }
        let flags = header[5];
        if flags & 0x02 != 0 && pages > 0 {
            report.push(Finding::warning(
                "OGG_BAD_PAGE",
                format!("偏移 {off} 处逻辑流中途出现 BOS 标志"),
                Some(off + 5),
            ));
        }
        let seg_count = usize::from(header[26]);
        let table_len = 27 + seg_count;
        let Some(table) = r.take(off + 27, seg_count) else {
            report.push(Finding::corrupt(
                "OGG_TRUNCATED",
                format!("偏移 {off} 处页分段表不完整（疑似截断）"),
                Some(off + 26),
            ));
            return;
        };
        let payload: usize = table.iter().map(|&b| usize::from(b)).sum();
        let page_len = (table_len + payload) as u64;
        if off + page_len > size {
            report.push(Finding::corrupt(
                "OGG_TRUNCATED",
                format!(
                    "偏移 {off} 处页声明 {page_len} 字节，超出文件范围（疑似截断）"
                ),
                Some(off),
            ));
            return;
        }
        let Some(mut page) = r.take(off, page_len as usize) else {
            report.push(Finding::corrupt(
                "OGG_TRUNCATED",
                format!("偏移 {off} 处页数据读取失败（疑似截断）"),
                Some(off),
            ));
            return;
        };
        let stored = u32::from_le_bytes(page[22..26].try_into().unwrap());
        for b in page[22..26].iter_mut() {
            *b = 0;
        }
        let computed = ogg_crc(&page);
        if stored != computed {
            report.push(Finding::corrupt(
                "OGG_BAD_CRC",
                format!("页 {pages} 的 CRC 不匹配：期望 {computed:08x}，实际 {stored:08x}"),
                Some(off + 22),
            ));
            return;
        }
        if flags & 0x04 != 0 {
            saw_eos = true;
        }
        pages += 1;
        off += page_len;
        if pages > 10_000_000 {
            break;
        }
        if page_len == 0 {
            break;
        }
    }

    if pages == 0 {
        report.push(Finding::corrupt(
            "OGG_NO_PAGE",
            "文件中没有完整的 Ogg 页",
            Some(0),
        ));
        return;
    }
    if off < size {
        report.push(Finding::corrupt(
            "OGG_BAD_PAGE",
            format!("文件尾部剩余 {} 字节不是完整的 Ogg 页", size - off),
            Some(off),
        ));
        return;
    }
    if !saw_eos {
        report.push(Finding::warning(
            "OGG_NO_EOS",
            "最后一页没有 EOS 标志，逻辑流可能不完整",
            Some(off.saturating_sub(27)),
        ));
    }
}

// ---------------------------------------------------------------------------
// FLV
// ---------------------------------------------------------------------------

fn verify_flv(path: &Path, size: u64, report: &mut Report) {
    let mut r = match Stream::open(path) {
        Ok(r) => r,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let Some(head) = r.take(0, 9.min(size as usize)) else {
        report.push(Finding::corrupt(
            "FLV_BAD_HEADER",
            "文件不足 9 字节，缺少 FLV 文件头",
            Some(0),
        ));
        return;
    };
    if head.len() < 9 || &head[0..3] != b"FLV" {
        report.push(Finding::corrupt(
            "FLV_BAD_HEADER",
            "缺少 FLV 签名，文件头已损坏",
            Some(0),
        ));
        return;
    }
    if head[3] != 1 {
        report.push(Finding::corrupt(
            "FLV_BAD_HEADER",
            format!("FLV 版本号 {} 非法", head[3]),
            Some(3),
        ));
        return;
    }
    let data_off = u32::from_be_bytes(head[5..9].try_into().unwrap()) as u64;
    if data_off < 9 || data_off > size {
        report.push(Finding::corrupt(
            "FLV_BAD_HEADER",
            format!("FLV 头长度字段 {data_off} 非法"),
            Some(5),
        ));
        return;
    }
    let Some(prev0) = r.u32(data_off) else {
        report.push(Finding::corrupt(
            "FLV_TRUNCATED",
            "缺少首标签前的 PreviousTagSize 字段",
            Some(data_off),
        ));
        return;
    };
    if prev0 != 0 {
        report.push(Finding::warning(
            "FLV_BAD_PREV_SIZE",
            format!("首个 PreviousTagSize 应为 0，实际 {prev0}"),
            Some(data_off),
        ));
    }

    let mut off = data_off + 4;
    let mut tags = 0u64;
    while off + 11 <= size {
        let Some(hdr) = r.take(off, 11) else {
            break;
        };
        let tag_type = hdr[0];
        if !matches!(tag_type, 0x08 | 0x09 | 0x12) {
            report.push(Finding::corrupt(
                "FLV_BAD_TAG",
                format!("偏移 {off} 处标签类型 0x{tag_type:02x} 非法，结构已损坏"),
                Some(off),
            ));
            return;
        }
        let data_size =
            (u32::from(hdr[1]) << 16) | (u32::from(hdr[2]) << 8) | u32::from(hdr[3]);
        let stream_id =
            (u32::from(hdr[8]) << 16) | (u32::from(hdr[9]) << 8) | u32::from(hdr[10]);
        if stream_id != 0 {
            report.push(Finding::corrupt(
                "FLV_BAD_TAG",
                format!("偏移 {off} 处 StreamID 应为 0，实际 {stream_id}"),
                Some(off + 8),
            ));
            return;
        }
        let tag_end = off + 11 + u64::from(data_size);
        if tag_end + 4 > size {
            report.push(Finding::corrupt(
                "FLV_TRUNCATED",
                format!(
                    "偏移 {off} 处标签声明 {data_size} 字节，超出文件范围（疑似截断）"
                ),
                Some(off),
            ));
            return;
        }
        if let Some(prev) = r.u32le(tag_end)
            && prev != 11 + data_size
        {
            report.push(Finding::warning(
                "FLV_BAD_PREV_SIZE",
                format!(
                    "偏移 {tag_end} 处 PreviousTagSize={prev}，应为 {}",
                    11 + data_size
                ),
                Some(tag_end),
            ));
        }
        off = tag_end + 4;
        tags += 1;
        if tags > 20_000_000 {
            break;
        }
    }

    if tags == 0 {
        report.push(Finding::corrupt(
            "FLV_NO_TAGS",
            "FLV 头之后没有任何标签，文件可能被截断",
            Some(data_off),
        ));
        return;
    }
    if off < size {
        report.push(Finding::corrupt(
            "FLV_TRUNCATED",
            format!("文件尾部剩余 {} 字节不是完整的标签", size - off),
            Some(off),
        ));
    }
}

// ---------------------------------------------------------------------------
// MPEG-TS
// ---------------------------------------------------------------------------

fn verify_ts(path: &Path, size: u64, report: &mut Report) {
    let mut r = match Stream::open(path) {
        Ok(r) => r,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let Some(head) = r.take(0, 1024.min(size as usize)) else {
        report.push(Finding::corrupt(
            "TS_BAD_SYNC",
            "文件为空，缺少 TS 同步字节",
            Some(0),
        ));
        return;
    };

    // 识别包长：188（标准）/ 192（M2TS，前 4 字节为时间码）/ 204（带 FEC）
    let stride: Option<(u64, u64)> = [(0u64, 188u64), (4, 192), (0, 204)]
        .into_iter()
        .find(|(base, pkt)| {
            let a = *base as usize;
            let b = a + *pkt as usize;
            head.len() > b && head[a] == 0x47 && head[b] == 0x47
        })
        .or_else(|| {
            if head.first() != Some(&0x47) {
                return None;
            }
            if size.is_multiple_of(188) {
                Some((0, 188))
            } else if size >= 192 && (size - 4).is_multiple_of(192) {
                Some((4, 192))
            } else if size.is_multiple_of(204) {
                Some((0, 204))
            } else {
                None
            }
        });

    let Some((base, pkt)) = stride else {
        report.push(Finding::corrupt(
            "TS_BAD_SYNC",
            "找不到 188/192/204 字节包同步字节 0x47，文件头已损坏",
            Some(0),
        ));
        return;
    };

    if size < base + pkt {
        report.push(Finding::corrupt(
            "TS_TRUNCATED",
            format!("文件只有 {size} 字节，不足一个 TS 包（{pkt} 字节）"),
            Some(0),
        ));
        return;
    }
    let payload_len = size - base;
    if !payload_len.is_multiple_of(pkt) {
        report.push(Finding::corrupt(
            "TS_TRUNCATED",
            format!(
                "文件长度不是 {pkt} 字节包的整数倍（余 {} 字节，疑似截断）",
                payload_len % pkt
            ),
            Some(size - payload_len % pkt),
        ));
        return;
    }

    let packets = payload_len / pkt;
    let chunk_pkts = 512u64;
    let mut buf = vec![0u8; chunk_pkts as usize * pkt as usize];
    let mut k = 0u64;
    while k < packets {
        let take = chunk_pkts.min(packets - k);
        let at = base + k * pkt;
        let want = take as usize * pkt as usize;
        let n = r.read_at(at, &mut buf[..want]);
        if n < want {
            report.push(Finding::corrupt(
                "TS_TRUNCATED",
                format!("偏移 {at} 处读取不到完整的 TS 包（疑似截断）"),
                Some(at),
            ));
            return;
        }
        let mut j = 0usize;
        while j < n {
            if buf[j] != 0x47 {
                report.push(Finding::corrupt(
                    "TS_BAD_SYNC",
                    format!(
                        "偏移 {} 处缺少同步字节 0x47（包结构已损坏）",
                        at + j as u64
                    ),
                    Some(at + j as u64),
                ));
                return;
            }
            j += pkt as usize;
        }
        k += take;
    }
}

// ---------------------------------------------------------------------------
// 深度解码
// ---------------------------------------------------------------------------

fn decode_code(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Mp4 => "MP4_DECODE_FAILED",
        MediaKind::Wav => "WAV_DECODE_FAILED",
        MediaKind::Avi => "AVI_DECODE_FAILED",
        MediaKind::Mkv | MediaKind::Webm => "MKV_DECODE_FAILED",
        MediaKind::Flv => "FLV_DECODE_FAILED",
        MediaKind::Ts => "TS_DECODE_FAILED",
        MediaKind::Mp3 => "MP3_DECODE_FAILED",
        MediaKind::Flac => "FLAC_DECODE_FAILED",
        MediaKind::Ogg => "OGG_DECODE_FAILED",
    }
}

/// 结构校验通过后执行的深度解码：容器交给 ffmpeg，音频流交给 symphonia
fn deep_decode(path: &Path, kind: MediaKind, opts: &Opts, report: &mut Report) {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    if !opts.should_deep(size) {
        return;
    }
    let full = size <= opts.max_deep_size;
    if kind.is_container() {
        crate::verifiers::tools::ffmpeg_verify(path, full, report, decode_code(kind), kind.label());
    } else {
        symphonia_decode(path, kind, full, report);
    }
}

fn symphonia_decode(path: &Path, kind: MediaKind, full: bool, report: &mut Report) {
    use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let code = decode_code(kind);
    let label = kind.label();

    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            io_error(report, e);
            return;
        }
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    );
    let mut format = match probed {
        Ok(p) => p.format,
        Err(_) => return, // 内置解码器不支持该流 → 降级为结构校验结论
    };

    let Some(track) = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
    else {
        return;
    };
    let track_id = track.id;
    let mut decoder = match symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
    {
        Ok(d) => d,
        Err(_) => return,
    };

    let budget: u64 = if full { u64::MAX } else { 2000 };
    let mut decoded: u64 = 0;
    loop {
        if decoded >= budget {
            break;
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 链式 OGG 流切换：视为正常结束
            Err(SymError::ResetRequired) => break,
            // 干净的 EOF（截断由结构校验负责）
            Err(SymError::IoError(_)) => break,
            Err(e) => {
                report.push(Finding::corrupt(
                    code,
                    format!("{label} 深度解码读取失败：{e}"),
                    None,
                ));
                return;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(_) => decoded += 1,
            Err(SymError::IoError(_)) => break,
            Err(SymError::DecodeError(e)) => {
                report.push(Finding::corrupt(
                    code,
                    format!("{label} 深度解码失败：{e}"),
                    None,
                ));
                return;
            }
            Err(e) => {
                report.push(Finding::corrupt(
                    code,
                    format!("{label} 深度解码失败：{e}"),
                    None,
                ));
                return;
            }
        }
    }
}



