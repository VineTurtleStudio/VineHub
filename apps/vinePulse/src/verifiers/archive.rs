use std::fs::File;
use std::io::{BufReader, Read};
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
