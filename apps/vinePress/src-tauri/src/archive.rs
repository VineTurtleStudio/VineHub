use crate::fsops::is_reparse;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveOutcome {
    /// created | skipped | cancelled | error
    pub status: String,
    pub archives: Vec<String>,
    pub message: String,
    pub total_bytes: u64,
}

pub struct ArchiveRequest {
    pub rar_path: String,
    pub src_dir: PathBuf,
    pub out_archive: PathBuf,
    pub password: Option<String>,
    pub encrypt_names: bool,
    pub volume_mib: u64,
    pub comment: Option<String>,
    pub overwrite: bool,
}

fn decode_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

fn archive_stem(out: &Path) -> String {
    out.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 是否属于该压缩包的产物（单卷 / 分卷）
fn belongs_to(name: &str, stem: &str) -> bool {
    if name == format!("{}.rar", stem) || name == format!("{}.zip", stem) {
        return true;
    }
    if name.starts_with(&format!("{}.part", stem)) && name.ends_with(".rar") {
        return true;
    }
    // 旧式分卷 name.r00 / name.r99
    let prefix = format!("{}.r", stem);
    if name.starts_with(&prefix) && name.len() == prefix.len() + 2 {
        return name[prefix.len()..].bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// 删除已存在的同名压缩包（含分卷）
fn remove_existing(out: &Path) -> Result<(), String> {
    for t in belonging_paths(out) {
        fs::remove_file(&t).map_err(|e| format!("删除 {} 失败: {}", t.display(), e))?;
    }
    Ok(())
}

fn belonging_paths(out: &Path) -> Vec<PathBuf> {
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let stem = archive_stem(out);
    let mut targets: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&parent) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if belongs_to(&name, &stem) {
                targets.push(e.path());
            }
        }
    }
    targets
}

fn backup_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".prevbak");
    PathBuf::from(os)
}

/// 把 out 的既有产物（含分卷）改名为 `*.prevbak`，返回 (原路径, 备份路径)
fn backup_existing(out: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut out_pairs = Vec::new();
    for p in belonging_paths(out) {
        let bak = backup_path(&p);
        if bak.exists() {
            fs::remove_file(&bak).map_err(|e| format!("清理旧备份失败: {}", e))?;
        }
        fs::rename(&p, &bak).map_err(|e| format!("备份 {} 失败: {}", p.display(), e))?;
        out_pairs.push((p, bak));
    }
    Ok(out_pairs)
}

fn restore_backup(pairs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    // 丢弃失败/取消产生的半成品
    if let Some(first) = pairs.first().map(|(p, _)| p.clone()) {
        for t in belonging_paths(&first) {
            let _ = fs::remove_file(t);
        }
    }
    for (orig, bak) in pairs {
        if !bak.exists() {
            continue;
        }
        if orig.exists() {
            fs::remove_file(orig).map_err(|e| format!("清理半成品失败: {}", e))?;
        }
        fs::rename(bak, orig).map_err(|e| format!("恢复 {} 失败: {}", orig.display(), e))?;
    }
    Ok(())
}

/// 在既有压缩包之上安全执行创建：
/// - `op` 返回 `Ok((value, true))`  → 保留新产物，删除备份
/// - `op` 返回 `Ok((value, false))` → 丢弃新产物并恢复备份（取消）
/// - `op` 返回 `Err`               → 丢弃新产物并恢复备份，再抛出错误
pub fn with_backup<T>(
    out: &Path,
    op: impl FnOnce() -> Result<(T, bool), String>,
) -> Result<T, String> {
    let pairs = backup_existing(out)?;
    match op() {
        Ok((value, true)) => {
            for (_, bak) in &pairs {
                let _ = fs::remove_file(bak);
            }
            Ok(value)
        }
        Ok((value, false)) => {
            restore_backup(&pairs)?;
            Ok(value)
        }
        Err(e) => {
            if let Err(re) = restore_backup(&pairs) {
                return Err(format!("{}（恢复备份时又出错：{}）", e, re));
            }
            Err(e)
        }
    }
}

/// 注释临时文件：所有返回路径都清理
struct CommentFile(Option<PathBuf>);

impl Drop for CommentFile {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

/// 调用 WinRAR/Rar 创建压缩包（支持密码、注释、分卷、进度、取消）
pub fn create_rar(
    req: &ArchiveRequest,
    progress: impl FnMut(u8),
    cancel: &AtomicBool,
) -> Result<ArchiveOutcome, String> {
    let exists = existing_archives(&req.out_archive);
    if !exists.is_empty() && !req.overwrite {
        return Ok(ArchiveOutcome {
            status: "skipped".into(),
            archives: exists,
            message: "已存在，跳过".into(),
            total_bytes: 0,
        });
    }
    if !req.src_dir.is_dir() {
        return Err(format!("源目录不存在: {}", req.src_dir.display()));
    }

    // 既有压缩包先改名备份，创建成功才删除，失败则原样恢复
    with_backup(&req.out_archive, || {
        let (outcome, commit) = create_rar_inner(req, progress, cancel)?;
        Ok((outcome, commit))
    })
}

fn create_rar_inner(
    req: &ArchiveRequest,
    mut progress: impl FnMut(u8),
    cancel: &AtomicBool,
) -> Result<(ArchiveOutcome, bool), String> {
    // 注释文件（WinRAR 的 -z 按本地 ANSI/GBK 读取）
    let mut comment_file = CommentFile(None);
    if let Some(text) = &req.comment {
        let dir = std::env::temp_dir().join("vinepress");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!(
            "comment_{}.txt",
            std::process::id() as u64 ^ req.out_archive.as_os_str().len() as u64
        ));
        let (bytes, _, _) = encoding_rs::GB18030.encode(text);
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        comment_file.0 = Some(path);
    }

    let mut args: Vec<String> = vec!["a".into(), "-r".into()];
    if let Some(pw) = &req.password {
        if !pw.is_empty() {
            if req.encrypt_names {
                args.push(format!("-hp{}", pw));
            } else {
                args.push(format!("-p{}", pw));
            }
        }
    }
    if req.volume_mib > 0 {
        args.push(format!("-v{}m", req.volume_mib));
    }
    if let Some(c) = &comment_file.0 {
        args.push(format!("-z{}", c.to_string_lossy()));
    }
    args.push("-xThumbs.db".into());
    args.push(req.out_archive.to_string_lossy().to_string());
    args.push("*".into());

    let mut child = Command::new(&req.rar_path)
        .args(&args)
        .current_dir(&req.src_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 {} 失败: {}", req.rar_path, e))?;

    let mut stderr_pipe = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stderr_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let mut stdout_pipe = child.stdout.take();
    let mut last_percent: u8 = 0;
    let mut out_buf: Vec<u8> = Vec::new();
    let mut cancelled = false;

    if let Some(p) = stdout_pipe.as_mut() {
        let mut chunk = [0u8; 8192];
        loop {
            if cancel.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            let n = match p.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            out_buf.extend_from_slice(&chunk[..n]);
            let text = decode_output(&out_buf[out_buf.len().saturating_sub(4096)..]);
            if let Some(pc) = last_percent_of(&text) {
                if pc != last_percent {
                    last_percent = pc;
                    progress(pc);
                }
            }
            if out_buf.len() > 4 * 1024 * 1024 {
                out_buf.drain(..out_buf.len() - 64 * 1024);
            }
        }
    }

    if cancelled {
        let _ = child.kill();
        let _ = child.wait();
        let _ = remove_existing(&req.out_archive);
        return Ok((
            ArchiveOutcome {
                status: "cancelled".into(),
                archives: vec![],
                message: "已取消".into(),
                total_bytes: 0,
            },
            false,
        ));
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr_buf = stderr_handle.join().unwrap_or_default();

    let code = status.code().unwrap_or(-1);
    let combined = decode_output(&out_buf) + &decode_output(&stderr_buf);
    if code != 0 && code != 1 {
        let _ = remove_existing(&req.out_archive);
        return Err(format!("Rar 退出码 {}: {}", code, tail(&combined, 600)));
    }

    let archives = existing_archives(&req.out_archive);
    if archives.is_empty() {
        return Err(format!("未生成压缩包: {}", tail(&combined, 600)));
    }
    progress(100);
    let total_bytes = archives
        .iter()
        .filter_map(|a| fs::metadata(a).ok())
        .map(|m| m.len())
        .sum();

    Ok((
        ArchiveOutcome {
            status: "created".into(),
            archives,
            message: String::new(),
            total_bytes,
        },
        true,
    ))
}

// ---------------------------------------------------------------- ZIP（内置）

enum ZipFail {
    Cancel,
    Fail(String),
}

/// 递归收集待打包文件：(绝对路径, zip 内路径, 大小)；跳过重联结点与 Thumbs.db
fn collect_files(root: &Path) -> Result<Vec<(PathBuf, String, u64)>, String> {
    let mut out: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let rd = fs::read_dir(&dir)
            .map_err(|e| format!("读取 {} 失败: {}", dir.display(), e))?;
        for entry in rd.flatten() {
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if is_reparse(&meta) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("Thumbs.db") {
                continue;
            }
            if meta.is_dir() {
                stack.push((entry.path(), format!("{}{}/", prefix, name)));
            } else if meta.is_file() {
                out.push((entry.path(), format!("{}{}", prefix, name), meta.len()));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

/// 用内置 zip 库创建压缩包（deflate，注释写 GBK 原始字节）
/// 密码 / 分卷不支持（由 preflight 提示），行为与 create_rar 对齐：
/// 已存在且不覆盖 → skipped；取消/失败 → 恢复既有压缩包
pub fn create_zip(
    req: &ArchiveRequest,
    progress: impl FnMut(u8),
    cancel: &AtomicBool,
) -> Result<ArchiveOutcome, String> {
    let exists = existing_archives(&req.out_archive);
    if !exists.is_empty() && !req.overwrite {
        return Ok(ArchiveOutcome {
            status: "skipped".into(),
            archives: exists,
            message: "已存在，跳过".into(),
            total_bytes: 0,
        });
    }
    if !req.src_dir.is_dir() {
        return Err(format!("源目录不存在: {}", req.src_dir.display()));
    }
    with_backup(&req.out_archive, || {
        let (outcome, commit) = create_zip_inner(req, progress, cancel)?;
        Ok((outcome, commit))
    })
}

fn create_zip_inner(
    req: &ArchiveRequest,
    mut progress: impl FnMut(u8),
    cancel: &AtomicBool,
) -> Result<(ArchiveOutcome, bool), String> {
    let files = collect_files(&req.src_dir)?;
    let total: u64 = files.iter().map(|f| f.2).sum();
    let mut written: u64 = 0;

    let built = (|| -> Result<(), ZipFail> {
        let file = fs::File::create(&req.out_archive)
            .map_err(|e| ZipFail::Fail(format!("创建输出失败: {}", e)))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        if let Some(text) = &req.comment {
            let (bytes, _, _) = encoding_rs::GB18030.encode(text);
            zip.set_raw_comment(bytes.into_owned().into_boxed_slice())
                .map_err(|e| ZipFail::Fail(format!("写入注释失败: {}", e)))?;
        }
        let mut buf = vec![0u8; 64 * 1024];
        for (abs, name, _) in &files {
            if cancel.load(Ordering::SeqCst) {
                return Err(ZipFail::Cancel);
            }
            zip.start_file(name.clone(), options)
                .map_err(|e| ZipFail::Fail(format!("写入条目 {} 失败: {}", name, e)))?;
            let mut input = fs::File::open(abs)
                .map_err(|e| ZipFail::Fail(format!("打开 {} 失败: {}", abs.display(), e)))?;
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Err(ZipFail::Cancel);
                }
                let n = input
                    .read(&mut buf)
                    .map_err(|e| ZipFail::Fail(format!("读取 {} 失败: {}", abs.display(), e)))?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buf[..n])
                    .map_err(|e| ZipFail::Fail(format!("写入 {} 失败: {}", name, e)))?;
                written += n as u64;
                let pc = if total == 0 {
                    100u8
                } else {
                    (written.saturating_mul(100) / total).min(100) as u8
                };
                progress(pc);
            }
        }
        zip.finish().map_err(|e| ZipFail::Fail(format!("收尾失败: {}", e)))?;
        Ok(())
    })();

    match built {
        Ok(()) => {
            progress(100);
            let archives = existing_archives(&req.out_archive);
            let total_bytes = archives
                .iter()
                .filter_map(|a| fs::metadata(a).ok())
                .map(|m| m.len())
                .sum();
            Ok((
                ArchiveOutcome {
                    status: "created".into(),
                    archives,
                    message: String::new(),
                    total_bytes,
                },
                true,
            ))
        }
        Err(ZipFail::Cancel) => {
            let _ = fs::remove_file(&req.out_archive);
            Ok((
                ArchiveOutcome {
                    status: "cancelled".into(),
                    archives: vec![],
                    message: "已取消".into(),
                    total_bytes: 0,
                },
                false,
            ))
        }
        Err(ZipFail::Fail(e)) => {
            let _ = fs::remove_file(&req.out_archive);
            Err(e)
        }
    }
}

fn existing_archives(out: &Path) -> Vec<String> {
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let stem = archive_stem(out);
    let mut list = Vec::new();
    if let Ok(rd) = fs::read_dir(&parent) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if belongs_to(&name, &stem) {
                list.push(e.path().to_string_lossy().to_string());
            }
        }
    }
    list.sort();
    list
}

fn last_percent_of(text: &str) -> Option<u8> {
    let mut best: Option<u8> = None;
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'%' {
            // 向前取数字
            let mut start = i;
            while start > 0 && bytes[start - 1].is_ascii_digit() {
                start -= 1;
            }
            if start < i {
                if let Ok(v) = text[start..i].parse::<u8>() {
                    best = Some(v);
                }
            }
        }
    }
    best
}

fn tail(text: &str, n: usize) -> String {
    let t = text.trim();
    if t.len() <= n {
        return t.to_string();
    }
    let mut start = t.len() - n;
    while start < t.len() && !t.is_char_boundary(start) {
        start += 1;
    }
    t[start..].to_string()
}

/// 简单延时（供轮询使用）
pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
