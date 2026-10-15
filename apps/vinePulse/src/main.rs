use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use vine_pulse::core::dispatch::Summary;
use vine_pulse::{Mode, Opts, Report, Severity, hash, walker};

const HELP: &str = "\
vine-pulse —— 给文件把脉，检测各类文件是否损坏

用法:
  vine-pulse                            启动图形界面（无参数时）
  vine-pulse [选项] <文件或目录>...     命令行模式

选项:
  -m, --manifest <文件>        哈希清单模式（.sha256 / .md5 / .sfv），
                               以清单中的文件为扫描目标并比对哈希
  -j, --json <输出文件>        导出 JSON 报告
  -d, --deep                   强制深度/核心校验（超过大小阈值的检查也执行）
  -F, --full                   全面检测模式：取消全部大小限制，核心校验、深度
                               解码、全量解码与内容级强化检查全部执行（慢但最彻底）
      --max-deep-size <MB>     深度解码大小阈值（默认 64；小于等于该值总是深度解码；
                               核心完整性校验（归档 CRC、SQLite/FLAC 校验）≤512 MiB 一律执行）
  -q, --quiet                  仅输出有问题的文件
      --entropy-threshold <f>  低熵判定阈值（0~8，默认 0.5）
      --no-color               关闭彩色输出
  -h, --help                   显示本帮助

退出码:
  0 = 全部正常   1 = 存在可疑/未知   2 = 存在确认损坏   3 = 参数或清单错误
";

struct Args {
    paths: Vec<PathBuf>,
    manifest: Option<PathBuf>,
    json: Option<PathBuf>,
    deep: bool,
    full: bool,
    max_deep_size_mb: Option<u64>,
    quiet: bool,
    no_color: bool,
    entropy_threshold: Option<f64>,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        paths: Vec::new(),
        manifest: None,
        json: None,
        deep: false,
        full: false,
        max_deep_size_mb: None,
        quiet: false,
        no_color: false,
        entropy_threshold: None,
        help: false,
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => args.help = true,
            "-d" | "--deep" => args.deep = true,
            "-F" | "--full" => args.full = true,
            "--max-deep-size" => {
                let v = it.next().ok_or("缺少 --max-deep-size 的参数值")?;
                let mb: u64 = v
                    .parse()
                    .map_err(|_| format!("无效的大小阈值：{v}（应为 MB 整数）"))?;
                args.max_deep_size_mb = Some(mb);
            }
            "-q" | "--quiet" => args.quiet = true,
            "--no-color" => args.no_color = true,
            "-m" | "--manifest" => {
                let v = it.next().ok_or("缺少 --manifest 的参数值")?;
                args.manifest = Some(PathBuf::from(v));
            }
            "-j" | "--json" => {
                let v = it.next().ok_or("缺少 --json 的参数值")?;
                args.json = Some(PathBuf::from(v));
            }
            "--entropy-threshold" => {
                let v = it.next().ok_or("缺少 --entropy-threshold 的参数值")?;
                let f: f64 = v
                    .parse()
                    .map_err(|_| format!("无效的阈值：{v}（应为 0~8 的数字）"))?;
                if !(0.0..=8.0).contains(&f) {
                    return Err(format!("阈值超出范围：{v}（应为 0~8）"));
                }
                args.entropy_threshold = Some(f);
            }
            other if other.starts_with('-') => {
                return Err(format!("未知选项：{other}"));
            }
            other => args.paths.push(PathBuf::from(other)),
        }
    }
    Ok(args)
}

fn severity_color(sev: Severity) -> &'static str {
    match sev {
        Severity::Ok => "\x1b[32m",
        Severity::Warning => "\x1b[33m",
        Severity::Corrupt => "\x1b[31m",
        Severity::Unknown => "\x1b[36m",
    }
}

/// Windows 控制台默认代码页多为 GBK，切到 UTF-8 以免中文输出乱码
#[cfg(windows)]
fn enable_utf8_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
    }
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

fn main() -> ExitCode {
    // argv 最早分流：rpc 模式跳过控制台 UTF-8 初始化与 CLI 解析，日志走 stderr
    if std::env::args().nth(1).as_deref() == Some("--rpc") {
        return vine_pulse::rpc::run();
    }

    // M3：无参数启动图形界面（带参数走现有 CLI，行为零回归）
    if std::env::args().len() == 1 {
        return match vine_pulse::ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("启动界面失败：{e}");
                ExitCode::from(3)
            }
        };
    }

    #[cfg(windows)]
    enable_utf8_console();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("错误：{e}");
            eprintln!("使用 --help 查看用法");
            return ExitCode::from(3);
        }
    };
    if args.help {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }

    let color = !args.no_color && std::io::stderr().is_terminal();

    let mut opts = Opts {
        deep: args.deep,
        full: args.full,
        ..Default::default()
    };
    if let Some(mb) = args.max_deep_size_mb {
        opts.max_deep_size = mb.saturating_mul(1024 * 1024);
    }
    if let Some(t) = args.entropy_threshold {
        opts.entropy_threshold = t;
    }

    // 扫描目标与哈希索引
    let (files, manifest_index) = match &args.manifest {
        Some(m) => {
            let entries = match hash::parse_manifest(m) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("错误：{e}");
                    return ExitCode::from(3);
                }
            };
            if !args.paths.is_empty() {
                eprintln!("提示：指定 --manifest 时以清单中的文件为扫描目标，位置参数已忽略");
            }
            let files: Vec<(PathBuf, u64)> = entries
                .iter()
                .map(|e| {
                    let size = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
                    (e.path.clone(), size)
                })
                .collect();
            let index = hash::index_manifest(&entries);
            opts.sidecar_hash = false;
            (files, Some(Arc::new(index)))
        }
        None => {
            if args.paths.is_empty() {
                eprintln!("错误：请指定至少一个文件或目录");
                eprintln!("使用 --help 查看用法");
                return ExitCode::from(3);
            }
            let (files, errors) = walker::collect_files(&args.paths);
            for e in &errors {
                eprintln!("警告：{e}");
            }
            (files, None)
        }
    };

    if files.is_empty() {
        eprintln!("错误：没有找到可扫描的文件");
        return ExitCode::from(3);
    }

    opts.manifest_index = manifest_index;

    let start = Instant::now();
    let cancel = AtomicBool::new(false);
    let show_progress = std::io::stderr().is_terminal();

    let reports = walker::scan(&files, &opts, &cancel, |done, total, _report| {
        if show_progress {
            eprint!("\r\x1b[K扫描中 {done}/{total}");
            let _ = std::io::stderr().flush();
        }
    });
    if show_progress {
        eprintln!();
    }

    let summary = Summary::from_reports(&reports);
    print_reports(&reports, &args, color);
    print_summary(&summary, start.elapsed());

    if let Some(json_path) = &args.json {
        match serde_json::to_string_pretty(&reports) {
            Ok(s) => match std::fs::write(json_path, s) {
                Ok(()) => eprintln!("JSON 报告已导出：{}", json_path.display()),
                Err(e) => eprintln!("导出 JSON 失败：{e}"),
            },
            Err(e) => eprintln!("序列化 JSON 失败：{e}"),
        }
    }

    if summary.corrupt > 0 {
        ExitCode::from(2)
    } else if summary.warning > 0 || summary.unknown > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_reports(reports: &[Report], args: &Args, color: bool) {
    for report in reports {
        if args.quiet && report.is_ok() {
            continue;
        }
        let sev = report.worst();
        let (label, c) = if color {
            (sev.label(), severity_color(sev))
        } else {
            (sev.label(), "")
        };
        let reset = if color { "\x1b[0m" } else { "" };
        let mode = match report.mode {
            Mode::Hash => "哈希",
            Mode::Blind => "盲检",
        };
        let name = report
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| report.path.to_string_lossy().into_owned());

        if report.is_ok() {
            println!("[{c}{label}{reset}] {name}  {} · {mode}", report.file_type);
        } else {
            println!(
                "[{c}{label}{reset}] {name}  {} · {mode} · {} 个问题",
                report.file_type,
                report.problem_count()
            );
        }

        for f in &report.findings {
            if f.severity == Severity::Ok && args.quiet {
                continue;
            }
            let (fc, fl) = if color {
                (severity_color(f.severity), f.severity.label())
            } else {
                ("", f.severity.label())
            };
            let offset = f
                .offset
                .map(|o| format!(" @ 偏移 0x{o:X}"))
                .unwrap_or_default();
            println!("    {fc}{fl}{reset} {}{offset}：", f.code);
            println!("        {}", f.message);
        }
    }
}

fn print_summary(s: &Summary, elapsed: std::time::Duration) {
    println!(
        "\n扫描完成：共 {} 个文件 | 正常 {} | 可疑 {} | 损坏 {} | 未知 {} | 耗时 {:.2}s",
        s.total,
        s.ok,
        s.warning,
        s.corrupt,
        s.unknown,
        elapsed.as_secs_f64()
    );
}
