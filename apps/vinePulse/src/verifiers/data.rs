use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Severity, Verifier};
use crate::verifiers::archive::{file_size, read_range};
use crate::verifiers::doc::check_text_encoding;

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

pub struct SqliteVerifier;

impl Verifier for SqliteVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Sqlite)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Sqlite))
    }

    fn verify(&self, path: &Path, opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "SQLite", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        if size < 16 {
            report.push(Finding::corrupt(
                "SQLITE_TRUNCATED",
                format!("文件只有 {size} 字节，不足 SQLite 16 字节文件头"),
                Some(0),
            ));
            return report;
        }
        let Some(head) = read_range(path, 0, 16, &mut report, "SQLite 头") else {
            return report;
        };
        if head.as_slice() != SQLITE_MAGIC {
            report.push(Finding::corrupt(
                "SQLITE_BAD_MAGIC",
                "文件头不是 \"SQLite format 3\\0\"",
                Some(0),
            ));
            return report;
        }

        // 完整性 quick_check 是 SQLite 的核心校验：≤ FULL_CHECK_LIMIT 一律执行
        if !opts.core_deep(size) {
            report.push(Finding::ok(
                "SQLITE_CHECK_SKIPPED",
                format!(
                    "文件 {size} 字节超过 {} MiB，PRAGMA quick_check 未执行（勾选「强制深度解码」可执行）",
                    crate::core::FULL_CHECK_LIMIT / (1024 * 1024)
                ),
            ));
            return report;
        }
        let conn = match rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(e) => {
                report.push(Finding::corrupt(
                    "SQLITE_OPEN",
                    format!("无法打开数据库：{e}"),
                    None,
                ));
                return report;
            }
        };
        match conn.prepare("PRAGMA quick_check") {
            Ok(mut stmt) => {
                let rows: Result<Vec<String>, _> = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .and_then(|it| it.collect());
                match rows {
                    Ok(rows) => {
                        if rows.len() == 1 && rows[0] == "ok" {
                            // 通过
                        } else {
                            report.push(Finding::corrupt(
                                "SQLITE_QUICK_CHECK",
                                format!("PRAGMA quick_check 报告问题：{}", rows.join("; ")),
                                None,
                            ));
                        }
                    }
                    Err(e) => {
                        report.push(Finding::corrupt(
                            "SQLITE_QUICK_CHECK",
                            format!("PRAGMA quick_check 执行失败：{e}"),
                            None,
                        ));
                    }
                }
            }
            Err(e) => {
                report.push(Finding::corrupt(
                    "SQLITE_QUICK_CHECK",
                    format!("无法执行 PRAGMA quick_check：{e}"),
                    None,
                ));
            }
        }
        report
    }
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

pub struct CsvVerifier;

impl Verifier for CsvVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic.is_none() && matches!(hint.ext.as_str(), "csv")
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "CSV", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size == 0 {
            return report;
        }
        let cap = 32 * 1024 * 1024;
        let Some(raw) = read_range(path, 0, size.min(cap) as usize, &mut report, "CSV 内容") else {
            return report;
        };
        check_text_encoding(&raw, &mut report);
        if report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Corrupt)
        {
            return report;
        }

        // 去掉 UTF-8 BOM 再解析
        let data = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &raw[3..]
        } else {
            &raw[..]
        };
        if data.iter().all(|&b| b.is_ascii_whitespace()) {
            report.push(Finding::warning("CSV_EMPTY", "文件没有可解析的内容", None));
            return report;
        }

        // 逐字节状态机：引号内忽略逗号/换行，"" 为转义引号
        let mut rows: Vec<usize> = Vec::new();
        let mut cols = 1usize;
        let mut in_quotes = false;
        let mut row_start = 0usize;
        let mut i = 0usize;
        while i < data.len() {
            let b = data[i];
            if in_quotes {
                if b == b'"' {
                    if data.get(i + 1) == Some(&b'"') {
                        i += 2;
                        continue;
                    }
                    in_quotes = false;
                }
                i += 1;
                continue;
            }
            match b {
                b'"' => in_quotes = true,
                b',' => cols += 1,
                b'\n' => {
                    if data[row_start..i].iter().any(|&c| c != b'\r') {
                        rows.push(cols);
                    }
                    cols = 1;
                    row_start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        if in_quotes {
            report.push(Finding::corrupt(
                "CSV_UNCLOSED_QUOTE",
                "引号未闭合（文件可能被截断）",
                Some(data.len() as u64),
            ));
            return report;
        }
        if data[row_start..].iter().any(|&c| c != b'\r') {
            rows.push(cols);
        }

        if rows.is_empty() {
            report.push(Finding::warning("CSV_EMPTY", "文件没有可解析的行", None));
            return report;
        }
        let header = rows[0];
        for (n, &c) in rows[1..].iter().enumerate() {
            if c != header {
                report.push(Finding::corrupt(
                    "CSV_RAGGED_ROW",
                    format!("第 {} 行有 {c} 列，与表头的 {header} 列不一致", n + 2),
                    None,
                ));
                return report;
            }
        }
        report
    }
}

// ---------------------------------------------------------------------------
// Parquet
// ---------------------------------------------------------------------------

pub struct ParquetVerifier;

impl Verifier for ParquetVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Parquet)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Parquet))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "Parquet", Mode::Blind);
        let Some(size) = file_size(path, &mut report) else {
            return report;
        };
        if size < 16 {
            report.push(Finding::corrupt(
                "PARQUET_TRUNCATED",
                format!("文件只有 {size} 字节，不足最小 Parquet 结构"),
                Some(0),
            ));
            return report;
        }
        let Some(head) = read_range(path, 0, 4, &mut report, "Parquet 头") else {
            return report;
        };
        if head != b"PAR1" {
            report.push(Finding::corrupt(
                "PARQUET_BAD_MAGIC",
                "文件头缺少 PAR1 魔数",
                Some(0),
            ));
            return report;
        }
        let Some(tail) = read_range(path, size - 8, 8, &mut report, "Parquet 尾") else {
            return report;
        };
        if tail[4..] != *b"PAR1" {
            report.push(Finding::corrupt(
                "PARQUET_BAD_MAGIC",
                "文件尾缺少 PAR1 魔数（可能被截断）",
                Some(size - 4),
            ));
            return report;
        }
        let footer_len = u32::from_le_bytes(tail[..4].try_into().unwrap());
        let Some(footer_start) = (size - 8).checked_sub(u64::from(footer_len)) else {
            report.push(Finding::corrupt(
                "PARQUET_BAD_FOOTER",
                format!("footer 长度 {footer_len} 超出文件范围"),
                Some(size - 8),
            ));
            return report;
        };
        if footer_start < 4 {
            report.push(Finding::corrupt(
                "PARQUET_BAD_FOOTER",
                format!("footer 起点 {footer_start} 侵入文件头魔数"),
                Some(size - 8),
            ));
            return report;
        }
        report
    }
}
