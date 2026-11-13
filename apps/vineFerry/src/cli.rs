use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use crate::core::conflict::ConflictPolicy;
use crate::core::copy::{run_transfer, TransferMode, TransferOptions};
use crate::core::hash::HashAlgo;
use crate::core::plan::{available_space, build_plan, concat, PlanError};
use crate::core::report::summary_to_csv;

const HELP: &str = r#"vine-ferry（藤渡）—— 文件复制/移动 + 转移后哈希校验

用法:
  vine-ferry                                 启动图形界面（无参数时）
  vine-ferry --rpc                           sidecar JSON-RPC 模式（宿主拉起）
  vine-ferry copy <源>... <目标目录> [选项]   复制并校验
  vine-ferry move <源>... <目标目录> [选项]   移动并校验（校验通过才删源）

选项:
  -c, --conflict <策略>   ask | skip | overwrite | merge | rename（默认 rename）
  -a, --algo <算法>        xxh3 | sha256 | crc32 | md5（默认 xxh3）
  -j, --json <输出文件>    导出 JSON 报告
      --csv <输出文件>     导出 CSV 报告
      --no-skip-identical 关闭同大小同时间快速跳过
  -h, --help               显示本帮助

退出码:
  0 = 全部成功   1 = 存在失败   2 = 参数错误
"#;

struct Args {
    mode: TransferMode,
    sources: Vec<PathBuf>,
    dest: PathBuf,
    conflict: ConflictPolicy,
    algo: HashAlgo,
    json: Option<PathBuf>,
    csv: Option<PathBuf>,
    skip_identical: bool,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mode = match it.next().as_deref() {
        Some("copy") => TransferMode::Copy,
        Some("move") => TransferMode::Move,
        Some("-h") | Some("--help") => {
            return Ok(Args {
                mode: TransferMode::Copy,
                sources: Vec::new(),
                dest: PathBuf::new(),
                conflict: ConflictPolicy::Rename,
                algo: HashAlgo::Xxh3,
                json: None,
                csv: None,
                skip_identical: true,
                help: true,
            });
        }
        Some(other) => return Err(format!("未知命令：{other}（应为 copy / move）")),
        None => return Err("缺少命令（copy / move）".into()),
    };

    let mut positional = Vec::new();
    let mut conflict = ConflictPolicy::Rename;
    let mut algo = HashAlgo::Xxh3;
    let mut json = None;
    let mut csv = None;
    let mut skip_identical = true;

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(Args {
                    mode,
                    sources: Vec::new(),
                    dest: PathBuf::new(),
                    conflict,
                    algo,
                    json,
                    csv,
                    skip_identical,
                    help: true,
                })
            }
            "-c" | "--conflict" => {
                let v = it.next().ok_or("缺少 --conflict 的参数值")?;
                conflict = match v.as_str() {
                    "ask" => ConflictPolicy::Ask,
                    "skip" => ConflictPolicy::Skip,
                    "overwrite" => ConflictPolicy::Overwrite,
                    "merge" => ConflictPolicy::Merge,
                    "rename" => ConflictPolicy::Rename,
                    other => return Err(format!("无效冲突策略：{other}")),
                };
            }
            "-a" | "--algo" => {
                let v = it.next().ok_or("缺少 --algo 的参数值")?;
                algo = HashAlgo::parse(&v).ok_or_else(|| format!("无效哈希算法：{v}"))?;
            }
            "-j" | "--json" => {
                let v = it.next().ok_or("缺少 --json 的参数值")?;
                json = Some(PathBuf::from(v));
            }
            "--csv" => {
                let v = it.next().ok_or("缺少 --csv 的参数值")?;
                csv = Some(PathBuf::from(v));
            }
            "--no-skip-identical" => skip_identical = false,
            other if other.starts_with('-') => return Err(format!("未知选项：{other}")),
            other => positional.push(PathBuf::from(other)),
        }
    }

    if positional.len() < 2 {
        return Err("至少需要一个源和一个目标目录".into());
    }
    let dest = positional.pop().expect("checked len >= 2");

    Ok(Args {
        mode,
        sources: positional,
        dest,
        conflict,
        algo,
        json,
        csv,
        skip_identical,
        help: false,
    })
}

pub fn run() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("错误：{e}");
            eprintln!("使用 --help 查看用法");
            return ExitCode::from(2);
        }
    };
    if args.help {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if !args.dest.is_dir() {
        eprintln!("错误：目标目录不存在：{}", args.dest.display());
        return ExitCode::from(2);
    }

    let mut plans = Vec::new();
    for src in &args.sources {
        if !src.exists() {
            eprintln!("错误：源不存在：{}", src.display());
            return ExitCode::from(2);
        }
        match build_plan(src, &args.dest, args.conflict) {
            Ok(p) => plans.push(p),
            Err(PlanError::UnresolvedConflicts(n)) => {
                eprintln!("错误：存在 {n} 个同名冲突，请用 --conflict 指定策略（skip/overwrite/merge/rename）");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("错误：{e}");
                return ExitCode::from(2);
            }
        }
    }
    let plan = concat(plans);
    if plan.items.is_empty() {
        eprintln!("错误：计划为空（没有可转移的文件）");
        return ExitCode::from(2);
    }
    if let Ok(free) = available_space(&args.dest)
        && plan.transfer_bytes > free
    {
        eprintln!(
            "错误：目标空间不足：需要 {} 字节，可用 {free} 字节",
            plan.transfer_bytes
        );
        return ExitCode::from(2);
    }

    eprintln!(
        "开始{}：{} 个文件，{} 字节 → {}",
        match args.mode {
            TransferMode::Copy => "复制",
            TransferMode::Move => "移动",
        },
        plan.items.len(),
        plan.transfer_bytes,
        args.dest.display()
    );

    let cancel = AtomicBool::new(false);
    let show_progress = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let summary = run_transfer(
        &plan,
        &TransferOptions {
            mode: args.mode,
            copy: crate::core::copy::CopyOptions {
                algo: args.algo,
                skip_identical: args.skip_identical,
                ..Default::default()
            },
        },
        |p| {
            if show_progress {
                eprint!("\r\x1b[K[{}/{}] {}", p.files_done, p.files_total, p.current);
            }
        },
        &cancel,
    );
    if show_progress {
        eprintln!();
    }

    println!(
        "完成：共 {} 个文件 | 成功 {} | 失败 {} | 跳过 {} | 取消 {} | 传输 {} 字节",
        summary.total, summary.ok, summary.bad, summary.skipped, summary.cancelled, summary.bytes
    );
    for o in &summary.outcomes {
        if o.status == crate::core::copy::FileStatus::Failed {
            eprintln!("  [失败] {}：{}", o.rel, o.message);
        }
    }

    if let Some(p) = &args.json {
        match crate::core::report::summary_to_json(&summary) {
            Ok(s) => match crate::core::report::write_report(p, &s) {
                Ok(()) => eprintln!("JSON 报告已导出：{}", p.display()),
                Err(e) => eprintln!("导出 JSON 失败：{e}"),
            },
            Err(e) => eprintln!("序列化 JSON 失败：{e}"),
        }
    }
    if let Some(p) = &args.csv {
        match crate::core::report::write_report(p, &summary_to_csv(&summary)) {
            Ok(()) => eprintln!("CSV 报告已导出：{}", p.display()),
            Err(e) => eprintln!("导出 CSV 失败：{e}"),
        }
    }

    if summary.bad > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
