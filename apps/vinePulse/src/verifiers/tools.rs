use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 设置页覆盖：`VINEPULSE_FFMPEG` / `VINEPULSE_FFPROBE` / `VINEPULSE_7Z` 环境变量优先
fn env_override(name: &str) -> Option<PathBuf> {
    let key = format!("VINEPULSE_{}", name.to_ascii_uppercase());
    let p = PathBuf::from(std::env::var_os(&key)?);
    p.is_file().then_some(p)
}

/// 外部工具查找顺序：设置页覆盖 → 程序同目录 → tools\<子目录> → PATH
fn locate(name: &str, sub: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join(name).with_extension("exe"));
        candidates.push(dir.join(name));
        candidates.push(dir.join("tools").join(sub).join(name).with_extension("exe"));
        candidates.push(dir.join("tools").join(sub).join(name));
    }
    candidates.extend(name_in_path(name));
    candidates.into_iter().find(|p| p.is_file())
}

fn name_in_path(name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(paths) = std::env::var_os("PATH") else {
        return out;
    };
    let exts: &[&str] = if cfg!(windows) { &["exe", ""] } else { &[""] };
    for dir in std::env::split_paths(&paths) {
        for ext in exts {
            let file = if ext.is_empty() {
                dir.join(name)
            } else {
                dir.join(name).with_extension(ext)
            };
            if file.is_file() {
                out.push(file);
            }
        }
    }
    out
}

fn ffmpeg_bin() -> Option<PathBuf> {
    static C: OnceLock<Option<PathBuf>> = OnceLock::new();
    env_override("ffmpeg").or_else(|| C.get_or_init(|| locate("ffmpeg", "ffmpeg")).clone())
}

fn ffprobe_bin() -> Option<PathBuf> {
    static C: OnceLock<Option<PathBuf>> = OnceLock::new();
    env_override("ffprobe").or_else(|| C.get_or_init(|| locate("ffprobe", "ffmpeg")).clone())
}

pub fn seven_zip() -> Option<PathBuf> {
    static C: OnceLock<Option<PathBuf>> = OnceLock::new();
    env_override("7z").or_else(|| C.get_or_init(|| locate("7z", "7z")).clone())
}

pub fn ffmpeg_available() -> bool {
    ffmpeg_bin().is_some() && ffprobe_bin().is_some()
}

pub struct RunOutcome {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl RunOutcome {
    pub fn ok(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }

    pub fn message(&self) -> String {
        let src = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        src.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("未知错误")
            .trim()
            .to_string()
    }
}

/// 带超时的子进程执行：stdout/stderr 由独立线程抽干，超时则 kill
fn run(mut cmd: Command, timeout: Duration) -> Option<RunOutcome> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();

    let h_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let h_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break None,
        }
    };

    let stdout = h_out.join().unwrap_or_default();
    let stderr = h_err.join().unwrap_or_default();
    Some(RunOutcome {
        code: status.and_then(|s| s.code()),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out: status.is_none(),
    })
}

fn ffprobe_duration(path: &Path) -> (Option<f64>, Option<RunOutcome>) {
    let Some(probe) = ffprobe_bin() else {
        return (None, None);
    };
    let mut cmd = Command::new(probe);
    cmd.args(["-v", "error", "-print_format", "json", "-show_format"])
        .arg(path);
    let Some(outcome) = run(cmd, Duration::from_secs(60)) else {
        return (None, None);
    };
    let duration = serde_json::from_str::<serde_json::Value>(&outcome.stdout)
        .ok()
        .and_then(|v| v.get("format")?.get("duration")?.as_str()?.parse::<f64>().ok());
    (duration, Some(outcome))
}

/// 深度解码（音视频容器，依赖 ffmpeg/ffprobe）
/// - `full` = 全量解码（`-v error -xerror`）
/// - 否则 quick：ffprobe 解析 + 首/中/尾抽样解码
pub fn ffmpeg_verify(
    path: &Path,
    full: bool,
    report: &mut crate::core::Report,
    code: &'static str,
    label: &str,
) {
    let (Some(ff), Some(_)) = (ffmpeg_bin(), ffprobe_bin()) else {
        return; // 未安装 → 交给结构校验兜底
    };

    if full {
        let mut cmd = Command::new(&ff);
        cmd.args(["-v", "error", "-xerror", "-i"])
            .arg(path)
            .args(["-f", "null", "-"]);
        match run(cmd, Duration::from_secs(600)) {
            None => {
                report.push(crate::core::Finding::ok(
                    "DECODE_SKIPPED",
                    format!("{label} 深度解码未执行（无法启动 ffmpeg），结构校验不受影响"),
                ));
            }
            Some(o) if o.timed_out => {
                report.push(crate::core::Finding::ok(
                    "DECODE_SKIPPED",
                    format!("{label} 深度解码超时未完成，结构校验不受影响"),
                ));
            }
            Some(o) if !o.ok() => {
                report.push(crate::core::Finding::corrupt(
                    code,
                    format!("{label} 深度解码失败：{}", o.message()),
                    None,
                ));
            }
            Some(_) => {}
        }
        return;
    }

    // quick 档：ffprobe 解析
    let (duration, probe) = ffprobe_duration(path);
    let Some(probe) = probe else {
        return;
    };
    if probe.timed_out {
        report.push(crate::core::Finding::ok(
            "DECODE_SKIPPED",
            format!("{label} ffprobe 解析超时未完成，结构校验不受影响"),
        ));
        return;
    }
    if !probe.ok() {
        report.push(crate::core::Finding::corrupt(
            code,
            format!("{label} 容器解析失败：{}", probe.message()),
            None,
        ));
        return;
    }

    // 首/中/尾抽样解码
    let mut offsets: Vec<f64> = vec![0.0];
    if let Some(d) = duration
        && d.is_finite()
        && d > 1.0
    {
        offsets.push(d * 0.5);
        offsets.push(d * 0.9);
    }
    for t in offsets {
        let mut cmd = Command::new(&ff);
        cmd.args(["-v", "error", "-xerror", "-ss"])
            .arg(format!("{t:.3}"))
            .arg("-i")
            .arg(path)
            .args(["-t", "0.5", "-f", "null", "-"]);
        let Some(o) = run(cmd, Duration::from_secs(120)) else {
            continue;
        };
        if o.timed_out {
            report.push(crate::core::Finding::ok(
                "DECODE_SKIPPED",
                format!("{label} 抽样解码超时（偏移 {t:.1}s），结构校验不受影响"),
            ));
            continue;
        }
        if !o.ok() {
            report.push(crate::core::Finding::corrupt(
                code,
                format!("{label} 偏移 {t:.1}s 处解码失败：{}", o.message()),
                None,
            ));
            return;
        }
    }
}
