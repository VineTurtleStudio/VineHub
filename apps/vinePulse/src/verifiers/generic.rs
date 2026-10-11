use std::path::Path;

use crate::core::dispatch::expected_by_ext;
use crate::core::entropy;
use crate::core::{FileHint, Finding, Mode, Opts, Report};

/// 无魔数的文本类扩展名
pub fn text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "csv"
            | "md"
            | "json"
            | "xml"
            | "html"
            | "htm"
            | "log"
            | "ini"
            | "cfg"
            | "conf"
            | "yaml"
            | "yml"
            | "toml"
            | "srt"
            | "ass"
            | "ssa"
            | "rs"
            | "py"
            | "js"
            | "ts"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "go"
            | "java"
            | "sh"
            | "bat"
            | "ps1"
            | "sql"
            | "gitignore"
    )
}

/// 头部字节是否像文本（不含 NUL，且无可打印字符占比高）
fn looks_like_text(head: &[u8]) -> bool {
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

/// 通用检查：空文件、魔数与扩展名不符、全零填充、低熵
pub fn verify(path: &Path, hint: &FileHint, opts: &Opts) -> Report {
    let mut report = Report::new(path.to_path_buf(), hint.type_name(), Mode::Blind);

    if hint.size == 0 {
        report.push(Finding::warning("EMPTY_FILE", "文件为空（0 字节）", None));
        return report;
    }

    // 魔数 vs 扩展名
    if let Some(expected) = expected_by_ext(&hint.ext) {
        match hint.magic {
            Some(actual) if actual != expected => {
                report.push(Finding::warning(
                    "MAGIC_MISMATCH",
                    format!(
                        "扩展名 .{} 期望 {}，实际识别为 {}",
                        hint.ext,
                        expected.name(),
                        actual.name()
                    ),
                    Some(0),
                ));
            }
            None => {
                let suffix = if looks_like_text(&hint.head) {
                    "内容疑似文本"
                } else {
                    "内容无法识别"
                };
                report.push(Finding::warning(
                    "MAGIC_MISMATCH",
                    format!(
                        "文件头不是 {} 的签名，{suffix}（扩展名与内容不符）",
                        expected.name()
                    ),
                    Some(0),
                ));
            }
            _ => {}
        }
    }

    // 熵分析：全零填充 / 过低熵
    match entropy::sample_file(path, hint.size) {
        Ok(sample) => {
            if entropy::all_zero(&sample) {
                report.push(Finding::warning(
                    "ALL_ZERO",
                    "采样内容全为零字节（疑似填充或未写入）",
                    Some(0),
                ));
            } else if hint.size >= 4096 {
                let e = entropy::shannon_entropy(&sample);
                if e < opts.entropy_threshold {
                    report.push(Finding::warning(
                        "LOW_ENTROPY",
                        format!(
                            "采样熵 {e:.2} 低于阈值 {:.2}（可能为重复填充）",
                            opts.entropy_threshold
                        ),
                        None,
                    ));
                }
            }
        }
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("读取文件做熵分析失败：{e}"),
                None,
            ));
        }
    }

    report
}
