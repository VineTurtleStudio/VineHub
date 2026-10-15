use std::path::Path;

use crate::core::dispatch::expected_by_ext;
use crate::core::entropy;
use crate::core::{FileHint, Finding, Mode, Opts, Report};

/// 重复填充判定阈值：采样中 top-2 字节值合计占比达到该值即报 REPEATED_FILL。
/// 正常内容到不了这个量级（文本约 15~30%，压缩/编码数据约 1%），故误报极低。
const REPEATED_FILL_RATIO: f64 = 0.90;

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
            } else if hint.size >= 8192 {
                // 小文件的采样字节太少，熵估计噪声大、天然偏低，容易误判为
                // 「重复填充」，故只对 ≥8 KiB 的文件做判定（全零填充由上面的
                // ALL_ZERO 单独提示，不受此阈值限制）。
                let e = entropy::shannon_entropy(&sample);
                if e < opts.entropy_threshold {
                    // 低熵只是提示：小文件、内容简单的文件天然低熵，保持 Ok 级
                    // 说明，不影响结论（真正可疑的填充走下面的 REPEATED_FILL）。
                    report.push(Finding::ok(
                        "LOW_ENTROPY",
                        format!(
                            "采样熵 {e:.2} 低于阈值 {:.2}（文件较小或内容重复度高，仅供参考）",
                            opts.entropy_threshold
                        ),
                    ));
                }
                // 重复填充：仅对无魔数的原始数据判定。有魔数的文件已由格式
                // 验证器做结构校验，且合法的双色图像（如 1-bit BMP）也呈双字节
                // 分布，会误报；无魔数时这种模式几乎必然是未写满/重复填充。
                if hint.magic.is_none() {
                    let (b1, b2, ratio) = entropy::top_two_ratio(&sample);
                    if ratio >= REPEATED_FILL_RATIO {
                        report.push(Finding::warning(
                            "REPEATED_FILL",
                            format!(
                                "采样中 0x{b1:02X}/0x{b2:02X} 两种字节合计占 {:.1}%（疑似重复填充或未写满）",
                                ratio * 100.0,
                            ),
                            None,
                        ));
                    }
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
