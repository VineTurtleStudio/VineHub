use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use crate::core::conflict::ConflictPolicy;
use crate::core::hash::{hash_file, HashAlgo};
use crate::core::paths::available_space;
use crate::core::plan::{build_plan, concat};
use crate::core::transfer::{
    run_transfer, CopyOptions, FileStatus, TransferMode, TransferOptions, VerifyMode,
};
use crate::core::verify::{parse_manifest, summarize, verify_against_manifest, verify_single};

const HELP: &str = r#"vineSeal（藤印）—— 文件复制/移动 + 转移后哈希校验

用法:
  vine-seal                                  启动图形界面（无参数时）
  vine-seal --rpc                            sidecar JSON-RPC 模式（宿主拉起）
  vine-seal copy <源>... <目标目录> [选项]    复制并校验
  vine-seal move <源>... <目标目录> [选项]    移动并校验（校验通过才删源）
  vine-seal verify <路径> --hash <期望哈希> [选项]     按期望哈希校验单个文件
  vine-seal verify -m <清单文件> <基目录> [选项]        按清单校验目录
  vine-seal hash <路径> [选项]               只计算并打印哈希

转移选项（copy / move）:
  -c, --conflict <策略>   skip | overwrite | rename（默认 rename）
  -a, --algo <算法>        sha256 | xxh3 | crc32 | md5（默认 sha256）
      --perfect           完美校验（额外复读源文件比对）
      --no-skip-identical 关闭同大小同时间快速跳过

校验选项（verify / hash）:
  -a, --algo <算法>        sha256 | xxh3 | crc32 | md5（默认 sha256）

退出码:
  0 = 全部成功   1 = 存在失败或校验不符   2 = 参数错误
"#;

pub fn run() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().map(String::as_str) else {
        eprintln!("错误：缺少命令（copy / move / verify / hash）");
        eprintln!("使用 --help 查看用法");
        return ExitCode::from(2);
    };
    let rest = &argv[1..];
    match cmd {
        "-h" | "--help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        "--version" => {
            println!("vine-seal {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "copy" => run_transfer_cmd(TransferMode::Copy, rest),
        "move" => run_transfer_cmd(TransferMode::Move, rest),
        "verify" => run_verify_cmd(rest),
        "hash" => run_hash_cmd(rest),
        other => {
            eprintln!("错误：未知命令 {other}（应为 copy / move / verify / hash）");
            ExitCode::from(2)
        }
    }
}

struct TransferArgs {
    sources: Vec<PathBuf>,
    dest: PathBuf,
    conflict: ConflictPolicy,
    algo: HashAlgo,
    perfect: bool,
    skip_identical: bool,
}

fn parse_transfer_args(rest: &[String]) -> Result<TransferArgs, String> {
    let mut positional = Vec::new();
    let mut conflict = ConflictPolicy::Rename;
    let mut algo = HashAlgo::Sha256;
    let mut perfect = false;
    let mut skip_identical = true;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-c" | "--conflict" => {
                let v = it.next().ok_or("缺少 --conflict 的参数值")?;
                conflict = match v.as_str() {
                    "skip" => ConflictPolicy::Skip,
                    "overwrite" => ConflictPolicy::Overwrite,
                    "rename" => ConflictPolicy::Rename,
                    other => return Err(format!("无效冲突策略：{other}（应为 skip/overwrite/rename）")),
                };
            }
            "-a" | "--algo" => {
                let v = it.next().ok_or("缺少 --algo 的参数值")?;
                algo = HashAlgo::parse(v).ok_or_else(|| format!("无效哈希算法：{v}"))?;
            }
            "--perfect" => perfect = true,
            "--no-skip-identical" => skip_identical = false,
            other if other.starts_with('-') => return Err(format!("未知选项：{other}")),
            other => positional.push(PathBuf::from(other)),
        }
    }
    if positional.len() < 2 {
        return Err("至少需要一个源和一个目标目录".into());
    }
    let dest = positional.pop().expect("checked len >= 2");
    Ok(TransferArgs {
        sources: positional,
        dest,
        conflict,
        algo,
        perfect,
        skip_identical,
    })
}

fn run_transfer_cmd(mode: TransferMode, rest: &[String]) -> ExitCode {
    let args = match parse_transfer_args(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("错误：{e}");
            eprintln!("使用 --help 查看用法");
            return ExitCode::from(2);
        }
    };
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
        "开始{}：{} 个条目，{} 字节 → {}",
        mode.label(),
        plan.items.len(),
        plan.transfer_bytes,
        args.dest.display()
    );

    let cancel = AtomicBool::new(false);
    let show_progress = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let summary = run_transfer(
        &plan,
        &TransferOptions {
            mode,
            copy: CopyOptions {
                algo: args.algo,
                verify: if args.perfect {
                    VerifyMode::Perfect
                } else {
                    VerifyMode::Conventional
                },
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
        "完成：共 {} 个条目 | 成功 {} | 失败 {} | 跳过 {} | 取消 {} | 传输 {} 字节",
        summary.total, summary.ok, summary.bad, summary.skipped, summary.cancelled, summary.bytes
    );
    for o in &summary.outcomes {
        if o.status == FileStatus::Failed {
            eprintln!("  [失败] {}：{}", o.rel, o.message);
        }
    }

    if summary.bad > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_verify_cmd(rest: &[String]) -> ExitCode {
    let mut manifest: Option<PathBuf> = None;
    let mut hash: Option<String> = None;
    let mut algo = HashAlgo::Sha256;
    let mut positional = Vec::new();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-m" | "--manifest" => {
                let Some(v) = it.next() else {
                    eprintln!("错误：缺少 -m 的参数值");
                    return ExitCode::from(2);
                };
                manifest = Some(PathBuf::from(v));
            }
            "--hash" => {
                let Some(v) = it.next() else {
                    eprintln!("错误：缺少 --hash 的参数值");
                    return ExitCode::from(2);
                };
                hash = Some(v.clone());
            }
            "-a" | "--algo" => {
                let Some(v) = it.next() else {
                    eprintln!("错误：缺少 --algo 的参数值");
                    return ExitCode::from(2);
                };
                match HashAlgo::parse(v) {
                    Some(a) => algo = a,
                    None => {
                        eprintln!("错误：无效哈希算法：{v}");
                        return ExitCode::from(2);
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("错误：未知选项：{other}");
                return ExitCode::from(2);
            }
            other => positional.push(PathBuf::from(other)),
        }
    }

    if let Some(mpath) = manifest {
        let Some(base) = positional.into_iter().next() else {
            eprintln!("错误：按清单校验需要提供基目录");
            return ExitCode::from(2);
        };
        if !mpath.is_file() {
            eprintln!("错误：清单不存在：{}", mpath.display());
            return ExitCode::from(2);
        }
        if !base.is_dir() {
            eprintln!("错误：基目录不存在：{}", base.display());
            return ExitCode::from(2);
        }
        let text = match std::fs::read_to_string(&mpath) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("错误：读取清单失败：{e}");
                return ExitCode::from(2);
            }
        };
        let entries = parse_manifest(&text);
        if entries.is_empty() {
            eprintln!("错误：清单为空或无法解析");
            return ExitCode::from(2);
        }
        let show_progress = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let items = verify_against_manifest(&entries, &base, algo, |done, total| {
            if show_progress {
                eprint!("\r\x1b[K[{done}/{total}]");
            }
        });
        if show_progress {
            eprintln!();
        }
        let (ok, missing, mismatch, read_error) = summarize(&items);
        println!(
            "校验完成：共 {} 项 | 一致 {} | 缺失 {} | 不一致 {} | 读取失败 {}",
            items.len(),
            ok,
            missing,
            mismatch,
            read_error
        );
        for it in &items {
            if it.status != crate::core::verify::VerifyStatus::Ok {
                eprintln!("  [{}] {}：{}", it.status.label(), it.path, it.message);
            }
        }
        if missing + mismatch + read_error > 0 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    } else {
        let Some(path) = positional.into_iter().next() else {
            eprintln!("错误：需要提供文件路径，或用 -m 指定清单");
            return ExitCode::from(2);
        };
        let Some(expected) = hash else {
            eprintln!("错误：需要 --hash 指定期望哈希值（或用 -m 指定清单）");
            return ExitCode::from(2);
        };
        let item = verify_single(&path, &expected, algo);
        println!("[{}] {}：{}", item.status.label(), item.path, item.message);
        if let (Some(e), Some(a)) = (&item.expected, &item.actual)
            && item.status != crate::core::verify::VerifyStatus::Ok
        {
            println!("  期望 {e}");
            println!("  实际 {a}");
        }
        if item.status == crate::core::verify::VerifyStatus::Ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    }
}

fn run_hash_cmd(rest: &[String]) -> ExitCode {
    let mut algo = HashAlgo::Sha256;
    let mut path: Option<PathBuf> = None;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-a" | "--algo" => {
                let Some(v) = it.next() else {
                    eprintln!("错误：缺少 --algo 的参数值");
                    return ExitCode::from(2);
                };
                match HashAlgo::parse(v) {
                    Some(a) => algo = a,
                    None => {
                        eprintln!("错误：无效哈希算法：{v}");
                        return ExitCode::from(2);
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("错误：未知选项：{other}");
                return ExitCode::from(2);
            }
            other => path = Some(PathBuf::from(other)),
        }
    }
    let Some(path) = path else {
        eprintln!("错误：需要提供文件路径");
        return ExitCode::from(2);
    };
    if !path.is_file() {
        eprintln!("错误：文件不存在：{}", path.display());
        return ExitCode::from(1);
    }
    match hash_file(algo, &path) {
        Ok(h) => {
            println!("{h}  {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("错误：读取失败：{e}");
            ExitCode::from(1)
        }
    }
}
