use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use filetime::FileTime;

use crate::core::hash::{hash_file, HashAlgo, Hasher};
use crate::core::paths::to_long_path;
use crate::core::plan::{ChangeKind, Plan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyMode {
    Conventional,
    Perfect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnVerifyFail {
    Report,
    Rename,
    Delete,
    RetryThenReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferMode {
    Copy,
    Move,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CopyOptions {
    pub algo: HashAlgo,
    pub verify: VerifyMode,
    pub preserve_mtime: bool,
    pub on_fail: OnVerifyFail,
    pub retries: u32,
    /// 目标已存在且大小、mtime 均一致时跳过（不做哈希，快速路径）。
    pub skip_identical: bool,
    /// 取消时保留 `*.ferry.tmp` 与续传 journal，源未变时下次从断点续写。
    pub resume: bool,
}

impl Default for CopyOptions {
    fn default() -> Self {
        CopyOptions {
            algo: HashAlgo::Xxh3,
            verify: VerifyMode::Conventional,
            preserve_mtime: true,
            on_fail: OnVerifyFail::RetryThenReport,
            retries: 3,
            skip_identical: true,
            resume: true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TransferOptions {
    pub copy: CopyOptions,
    pub mode: TransferMode,
}

impl Default for TransferOptions {
    fn default() -> Self {
        TransferOptions {
            copy: CopyOptions::default(),
            mode: TransferMode::Copy,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Verified,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileOutcome {
    pub rel: String,
    pub size: u64,
    pub src_hash: Option<String>,
    pub dst_hash: Option<String>,
    pub verified: bool,
    pub status: FileStatus,
    pub message: String,
}

#[derive(Debug)]
pub enum CopyError {
    Cancelled,
    Io(std::io::Error),
    VerifyMismatch { src: String, dst: String },
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::Cancelled => write!(f, "已取消"),
            CopyError::Io(e) => write!(f, "IO 错误：{e}"),
            CopyError::VerifyMismatch { src, dst } => {
                write!(f, "校验不一致（源 {src} / 目标 {dst}）")
            }
        }
    }
}

impl std::error::Error for CopyError {}

impl From<std::io::Error> for CopyError {
    fn from(e: std::io::Error) -> Self {
        CopyError::Io(e)
    }
}

pub const TMP_SUFFIX: &str = ".ferry.tmp";
pub const VERIFY_ERR_SUFFIX: &str = ".verify_err";
pub const RESUME_SUFFIX: &str = ".ferry.resume";

fn tmp_path(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    dst.with_file_name(format!("{name}{TMP_SUFFIX}"))
}

fn journal_path(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    dst.with_file_name(format!("{name}{RESUME_SUFFIX}"))
}

fn write_journal(dst: &Path, src_meta: &fs::Metadata) -> bool {
    let Ok(mtime) = src_meta.modified() else {
        return false;
    };
    let Ok(dur) = mtime.duration_since(SystemTime::UNIX_EPOCH) else {
        return false;
    };
    let body = format!("{}\n{}\n", src_meta.len(), dur.as_secs());
    fs::write(to_long_path(&journal_path(dst)), body).is_ok()
}

fn try_resume_offset(src: &Path, tmp: &Path, dst: &Path) -> Option<u64> {
    let text = fs::read_to_string(to_long_path(&journal_path(dst))).ok()?;
    let mut lines = text.lines();
    let size: u64 = lines.next()?.parse().ok()?;
    let msecs: u64 = lines.next()?.parse().ok()?;
    let meta = fs::symlink_metadata(src).ok()?;
    if !meta.is_file() || meta.len() != size {
        return None;
    }
    let cur = meta
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if cur != msecs {
        return None;
    }
    let tmp_meta = fs::symlink_metadata(tmp).ok()?;
    let off = tmp_meta.len();
    if off == 0 || off > size {
        return None;
    }
    Some(off)
}

fn verify_err_path(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    dst.with_file_name(format!("{name}{VERIFY_ERR_SUFFIX}"))
}

fn cleanup(path: &Path) {
    let _ = fs::remove_file(to_long_path(path));
}

fn copy_once(
    src: &Path,
    dst: &Path,
    opts: &CopyOptions,
    on_bytes: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(String, String), CopyError> {
    let meta = fs::symlink_metadata(src)?;
    let src_long = to_long_path(src);
    let tmp = tmp_path(dst);
    let tmp_long = to_long_path(&tmp);

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(to_long_path(parent))?;
    }

    let resume_offset = if opts.resume {
        try_resume_offset(src, &tmp, dst).unwrap_or(0)
    } else {
        0
    };
    let mut hasher = Hasher::new(opts.algo);
    let mut reader = File::open(&src_long)?;
    let mut writer;
    if resume_offset > 0 {
        let mut remaining = resume_offset;
        let mut buf = vec![0u8; 128 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = reader.read(&mut buf[..want])?;
            if n == 0 {
                return Err(CopyError::Io(std::io::Error::other(
                    "续传偏移超过源文件长度",
                )));
            }
            hasher.update(&buf[..n]);
            remaining -= n as u64;
        }
        writer = OpenOptions::new().append(true).open(&tmp_long)?;
        on_bytes(resume_offset);
    } else {
        cleanup(&tmp);
        writer = File::create(&tmp_long)?;
    }

    let mut buf = vec![0u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(writer);
            if opts.resume {
                write_journal(dst, &meta);
            } else {
                cleanup(&tmp);
            }
            return Err(CopyError::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        on_bytes(n as u64);
    }
    writer.flush()?;
    writer.sync_all()?;
    drop(writer);

    if opts.preserve_mtime
        && let Ok(modified) = meta.modified()
    {
        let ft = FileTime::from_system_time(modified);
        filetime::set_file_mtime(&tmp_long, ft)?;
    }

    let src_hash = hasher.finalize_hex();
    let dst_hash = hash_file(opts.algo, &tmp)?;
    if dst_hash != src_hash {
        return Err(CopyError::VerifyMismatch {
            src: src_hash,
            dst: dst_hash,
        });
    }

    if opts.verify == VerifyMode::Perfect {
        let src_rehash = hash_file(opts.algo, src)?;
        if src_rehash != src_hash {
            return Err(CopyError::VerifyMismatch {
                src: src_rehash,
                dst: src_hash,
            });
        }
    }

    if dst.exists() {
        fs::remove_file(to_long_path(dst))?;
    }
    fs::rename(&tmp_long, to_long_path(dst))?;
    cleanup(&journal_path(dst));

    if opts.preserve_mtime
        && let Ok(modified) = meta.modified()
    {
        let ft = FileTime::from_system_time(modified);
        let _ = filetime::set_file_mtime(to_long_path(dst), ft);
    }

    Ok((src_hash, dst_hash))
}

fn identical_fast_path(src: &Path, dst: &Path) -> bool {
    let (Ok(sm), Ok(dm)) = (fs::symlink_metadata(src), fs::symlink_metadata(dst)) else {
        return false;
    };
    if !sm.is_file() || !dm.is_file() || sm.len() != dm.len() {
        return false;
    }
    match (sm.modified(), dm.modified()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn copy_file(
    src: &Path,
    dst: &Path,
    opts: &CopyOptions,
    mut on_bytes: impl FnMut(u64),
    cancel: &AtomicBool,
) -> Result<FileOutcome, CopyError> {
    let size = fs::symlink_metadata(src)?.len();
    let rel = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if opts.skip_identical && identical_fast_path(src, dst) {
        return Ok(FileOutcome {
            rel,
            size,
            src_hash: None,
            dst_hash: None,
            verified: false,
            status: FileStatus::Skipped,
            message: "目标与源大小、修改时间一致，快速跳过".into(),
        });
    }
    let mut last_err = None;
    for attempt in 0..=opts.retries {
        if cancel.load(Ordering::Relaxed) {
            return Ok(FileOutcome {
                rel,
                size,
                src_hash: None,
                dst_hash: None,
                verified: false,
                status: FileStatus::Cancelled,
                message: "已取消".into(),
            });
        }
        match copy_once(src, dst, opts, &mut on_bytes, cancel) {
            Ok((src_hash, dst_hash)) => {
                return Ok(FileOutcome {
                    rel,
                    size,
                    src_hash: Some(src_hash),
                    dst_hash: Some(dst_hash),
                    verified: true,
                    status: FileStatus::Verified,
                    message: format!("第 {} 次尝试成功", attempt + 1),
                })
            }
            Err(CopyError::Cancelled) => {
                return Ok(FileOutcome {
                    rel,
                    size,
                    src_hash: None,
                    dst_hash: None,
                    verified: false,
                    status: FileStatus::Cancelled,
                    message: "已取消".into(),
                })
            }
            Err(e) => {
                let keep_for_rename = matches!(e, CopyError::VerifyMismatch { .. })
                    && opts.on_fail == OnVerifyFail::Rename;
                let keep_for_resume = matches!(e, CopyError::Cancelled) && opts.resume;
                if !keep_for_rename && !keep_for_resume {
                    cleanup(&tmp_path(dst));
                    cleanup(&journal_path(dst));
                }
                last_err = Some(e);
            }
        }
    }

    let err = last_err.unwrap_or_else(|| {
        CopyError::Io(std::io::Error::other("未知错误"))
    });
    match (&err, opts.on_fail) {
        (CopyError::VerifyMismatch { src: s, dst: d }, OnVerifyFail::Rename) => {
            let tmp = tmp_path(dst);
            if tmp.exists() {
                let _ = fs::rename(to_long_path(&tmp), to_long_path(&verify_err_path(dst)));
            }
            Ok(FileOutcome {
                rel,
                size,
                src_hash: Some(s.clone()),
                dst_hash: Some(d.clone()),
                verified: false,
                status: FileStatus::Failed,
                message: "校验不一致，目标已保存为 .verify_err".into(),
            })
        }
        _ => Ok(FileOutcome {
            rel,
            size,
            src_hash: None,
            dst_hash: None,
            verified: false,
            status: FileStatus::Failed,
            message: err.to_string(),
        }),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub total: u32,
    pub ok: u32,
    pub bad: u32,
    pub skipped: u32,
    pub cancelled: u32,
    pub bytes: u64,
    pub outcomes: Vec<FileOutcome>,
}

fn remove_empty_parents(start: &Path, stop_at: &Path) {
    let mut cur = start.to_path_buf();
    while cur.starts_with(stop_at) && cur != stop_at {
        if fs::remove_dir(&cur).is_err() {
            break;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }
}

pub fn run_transfer(
    plan: &Plan,
    opts: &TransferOptions,
    mut on_progress: impl FnMut(&Progress),
    cancel: &AtomicBool,
) -> RunSummary {
    let files_total = plan.items.len() as u32;
    let bytes_total = plan.transfer_bytes;
    let mut summary = RunSummary {
        total: files_total,
        ok: 0,
        bad: 0,
        skipped: 0,
        cancelled: 0,
        bytes: 0,
        outcomes: Vec::new(),
    };
    let mut bytes_done = 0u64;

    for (idx, item) in plan.items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled += files_total - idx as u32;
            break;
        }
        if item.kind == ChangeKind::Skip {
            summary.skipped += 1;
            summary.outcomes.push(FileOutcome {
                rel: item.rel.clone(),
                size: item.size,
                src_hash: None,
                dst_hash: None,
                verified: false,
                status: FileStatus::Skipped,
                message: "按冲突策略跳过".into(),
            });
            on_progress(&Progress {
                files_done: idx as u32 + 1,
                files_total,
                bytes_done,
                bytes_total,
                current: item.rel.clone(),
            });
            continue;
        }

        on_progress(&Progress {
            files_done: idx as u32,
            files_total,
            bytes_done,
            bytes_total,
            current: item.rel.clone(),
        });

        let outcome = if item.is_dir {
            // 目录项：只创建目录（空文件夹也落地），不搬数据。
            match fs::create_dir_all(to_long_path(&item.dest)) {
                Ok(()) => FileOutcome {
                    rel: item.rel.clone(),
                    size: 0,
                    src_hash: None,
                    dst_hash: None,
                    verified: true,
                    status: FileStatus::Verified,
                    message: "已创建文件夹".into(),
                },
                Err(e) => FileOutcome {
                    rel: item.rel.clone(),
                    size: 0,
                    src_hash: None,
                    dst_hash: None,
                    verified: false,
                    status: FileStatus::Failed,
                    message: format!("创建文件夹失败：{e}"),
                },
            }
        } else if opts.mode == TransferMode::Move
            && crate::core::paths::same_drive(&item.src, &item.dest)
        {
            move_by_rename(&item.src, &item.dest, &item.rel)
        } else {
            match copy_file(
                &item.src,
                &item.dest,
                &opts.copy,
                |n| bytes_done += n,
                cancel,
            ) {
                Ok(out) => {
                    if out.verified && opts.mode == TransferMode::Move {
                        let _ = fs::remove_file(to_long_path(&item.src));
                        if let Some(p) = item.src.parent() {
                            remove_empty_parents(p, &plan.src_root);
                        }
                    }
                    out
                }
                Err(e) => FileOutcome {
                    rel: item.rel.clone(),
                    size: item.size,
                    src_hash: None,
                    dst_hash: None,
                    verified: false,
                    status: if matches!(e, CopyError::Cancelled) {
                        FileStatus::Cancelled
                    } else {
                        FileStatus::Failed
                    },
                    message: e.to_string(),
                },
            }
        };

        match outcome.status {
            FileStatus::Verified => {
                summary.ok += 1;
                summary.bytes += item.size;
            }
            FileStatus::Skipped => summary.skipped += 1,
            FileStatus::Cancelled => summary.cancelled += 1,
            FileStatus::Failed => summary.bad += 1,
        }
        summary.outcomes.push(outcome);
    }

    // 移动模式：全部成功时自底向上清理已空的源目录（含被移走的文件夹本身）。
    if opts.mode == TransferMode::Move && summary.bad == 0 && summary.cancelled == 0 {
        for item in plan.items.iter().rev() {
            if item.is_dir {
                let _ = fs::remove_dir(to_long_path(&item.src));
            }
        }
    }

    on_progress(&Progress {
        files_done: summary.ok + summary.bad + summary.skipped + summary.cancelled,
        files_total,
        bytes_done,
        bytes_total,
        current: String::new(),
    });
    summary
}

fn move_by_rename(src: &Path, dst: &Path, rel: &str) -> FileOutcome {
    let size = fs::symlink_metadata(src).map(|m| m.len()).unwrap_or(0);
    if let Some(parent) = dst.parent()
        && let Err(e) = fs::create_dir_all(to_long_path(parent))
    {
        return FileOutcome {
            rel: rel.to_string(),
            size,
            src_hash: None,
            dst_hash: None,
            verified: false,
            status: FileStatus::Failed,
            message: format!("创建目标目录失败：{e}"),
        };
    }
    let renamed = fs::rename(to_long_path(src), to_long_path(dst));
    match renamed {
        Ok(_) => FileOutcome {
            rel: rel.to_string(),
            size,
            src_hash: None,
            dst_hash: None,
            verified: true,
            status: FileStatus::Verified,
            message: "同盘移动（rename 快路径，数据未搬动）".into(),
        },
        Err(e) => FileOutcome {
            rel: rel.to_string(),
            size,
            src_hash: None,
            dst_hash: None,
            verified: false,
            status: FileStatus::Failed,
            message: format!("移动失败：{e}"),
        },
    }
}

pub fn mtime_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn opts() -> CopyOptions {
        CopyOptions {
            algo: HashAlgo::Sha256,
            verify: VerifyMode::Conventional,
            preserve_mtime: true,
            on_fail: OnVerifyFail::Report,
            retries: 1,
            skip_identical: true,
            resume: false,
        }
    }

    #[test]
    fn identical_size_mtime_fast_skips() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"same content").unwrap();
        let old = FileTime::from_unix_time(1_600_000_000, 0);
        filetime::set_file_mtime(&src, old).unwrap();
        let dst = t.path().join("b.bin");
        fs::write(&dst, b"same content").unwrap();
        filetime::set_file_mtime(&dst, old).unwrap();
        let out = copy_file(&src, &dst, &opts(), |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.status, FileStatus::Skipped);
        assert!(!out.verified);
        assert!(out.message.contains("快速跳过"));
        assert_eq!(fs::read(&dst).unwrap(), b"same content");
    }

    #[test]
    fn different_mtime_does_not_fast_skip() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"same content").unwrap();
        filetime::set_file_mtime(&src, FileTime::from_unix_time(1_600_000_000, 0)).unwrap();
        let dst = t.path().join("b.bin");
        fs::write(&dst, b"same content").unwrap();
        filetime::set_file_mtime(&dst, FileTime::from_unix_time(1_700_000_000, 0)).unwrap();
        let out = copy_file(&src, &dst, &opts(), |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.status, FileStatus::Verified);
    }

    #[test]
    fn skip_identical_disabled_copies_anyway() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"same content").unwrap();
        let old = FileTime::from_unix_time(1_600_000_000, 0);
        filetime::set_file_mtime(&src, old).unwrap();
        let dst = t.path().join("b.bin");
        fs::write(&dst, b"same content").unwrap();
        filetime::set_file_mtime(&dst, old).unwrap();
        let mut o = opts();
        o.skip_identical = false;
        let out = copy_file(&src, &dst, &o, |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.status, FileStatus::Verified);
    }

    #[test]
    fn copy_roundtrip_verified() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        let data = vec![42u8; 500_000];
        fs::write(&src, &data).unwrap();
        let dst = t.path().join("b.bin");
        let out = copy_file(&src, &dst, &opts(), |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.status, FileStatus::Verified);
        assert!(out.verified);
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert_eq!(out.src_hash, out.dst_hash);
    }

    #[test]
    fn copy_preserves_mtime() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"x").unwrap();
        let old = FileTime::from_unix_time(1_600_000_000, 0);
        filetime::set_file_mtime(&src, old).unwrap();
        let dst = t.path().join("b.bin");
        copy_file(&src, &dst, &opts(), |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(FileTime::from_system_time(mtime_of(&dst).unwrap()), old);
    }

    #[test]
    fn no_tmp_left_after_success() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"data").unwrap();
        let dst = t.path().join("b.bin");
        copy_file(&src, &dst, &opts(), |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(!t.path().join(format!("b.bin{TMP_SUFFIX}")).exists());
    }

    #[test]
    fn pre_cancelled_returns_cancelled() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"data").unwrap();
        let dst = t.path().join("b.bin");
        let out = copy_file(
            &src,
            &dst,
            &opts(),
            |_| {},
            &AtomicBool::new(true),
        )
        .unwrap();
        assert_eq!(out.status, FileStatus::Cancelled);
        assert!(!dst.exists());
    }

    #[test]
    fn invalid_dest_fails_and_source_kept() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.bin");
        fs::write(&src, b"data").unwrap();
        let blocker = t.path().join("blocker");
        fs::write(&blocker, b"i am a file").unwrap();
        let dst = blocker.join("sub/b.bin");
        let out = copy_file(&src, &dst, &opts(), |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.status, FileStatus::Failed);
        assert!(src.exists());
    }

    #[test]
    fn move_same_drive_renames_and_keeps_content() {
        let t = tempfile::tempdir().unwrap();
        let src_dir = t.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("a.bin");
        fs::write(&src, b"payload").unwrap();
        let dst_dir = t.path().join("dst");
        fs::create_dir_all(&dst_dir).unwrap();
        let dst = dst_dir.join("a.bin");
        let out = move_by_rename(&src, &dst, "a.bin");
        assert_eq!(out.status, FileStatus::Verified);
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
    }

    #[test]
    fn run_transfer_copy_all_ok() {
        let t = tempfile::tempdir().unwrap();
        let src_dir = t.path().join("src");
        fs::create_dir_all(src_dir.join("sub")).unwrap();
        fs::write(src_dir.join("a.txt"), b"aaaaa").unwrap();
        fs::write(src_dir.join("sub/b.txt"), b"bb").unwrap();
        let dst = t.path().join("dst");
        fs::create_dir_all(&dst).unwrap();
        let plan = crate::core::plan::build_plan(&src_dir, &dst, crate::core::conflict::ConflictPolicy::Ask)
            .unwrap();
        let summary = run_transfer(
            &plan,
            &TransferOptions::default(),
            |_| {},
            &AtomicBool::new(false),
        );
        // src 目录 + a.txt + sub 目录 + sub/b.txt
        assert_eq!(summary.ok, 4);
        assert_eq!(summary.bad, 0);
        // 文件夹结构保留：目标是 dst/src/…，而不是打散到 dst 根部
        assert!(dst.join("src/a.txt").exists());
        assert!(dst.join("src/sub/b.txt").exists());
        assert!(src_dir.join("a.txt").exists());
    }

    #[test]
    fn run_transfer_move_deletes_source_after_verify() {
        let t = tempfile::tempdir().unwrap();
        let src_dir = t.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("a.txt"), b"aaaaa").unwrap();
        let dst = t.path().join("dst");
        fs::create_dir_all(&dst).unwrap();
        let plan = crate::core::plan::build_plan(&src_dir, &dst, crate::core::conflict::ConflictPolicy::Ask)
            .unwrap();
        let summary = run_transfer(
            &plan,
            &TransferOptions {
                copy: CopyOptions::default(),
                mode: TransferMode::Move,
            },
            |_| {},
            &AtomicBool::new(false),
        );
        // src 目录项 + a.txt
        assert_eq!(summary.ok, 2);
        assert!(dst.join("src/a.txt").exists());
        assert!(!src_dir.join("a.txt").exists());
        // 源文件夹已空，随移动一并移除
        assert!(!src_dir.exists());
    }

    #[test]
    fn run_transfer_cancel_keeps_source() {
        let t = tempfile::tempdir().unwrap();
        let src_dir = t.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("a.txt"), b"aaaaa").unwrap();
        let dst = t.path().join("dst");
        fs::create_dir_all(&dst).unwrap();
        let plan = crate::core::plan::build_plan(&src_dir, &dst, crate::core::conflict::ConflictPolicy::Ask)
            .unwrap();
        let cancel = AtomicBool::new(true);
        let summary = run_transfer(&plan, &TransferOptions::default(), |_| {}, &cancel);
        assert!(src_dir.join("a.txt").exists());
        assert_eq!(summary.ok, 0);
    }

    #[test]
    fn cancel_keeps_tmp_and_journal_for_resume() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("big.bin");
        let data = vec![9u8; 400_000];
        fs::write(&src, &data).unwrap();
        let dst = t.path().join("out.bin");
        let mut o = opts();
        o.resume = true;
        let cancel = AtomicBool::new(false);
        let mut written = 0u64;
        let out = copy_file(
            &src,
            &dst,
            &o,
            |n| {
                written += n;
                if written >= 128 * 1024 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(out.status, FileStatus::Cancelled);
        assert!(!dst.exists());
        let tmp = t.path().join(format!("out.bin{TMP_SUFFIX}"));
        let journal = t.path().join(format!("out.bin{RESUME_SUFFIX}"));
        assert!(tmp.exists());
        assert!(journal.exists());
        let tmp_len = fs::metadata(&tmp).unwrap().len();
        assert!(tmp_len > 0 && tmp_len < data.len() as u64);
        // 恢复复制：同源同目标应续写并最终校验通过
        let out2 = copy_file(&src, &dst, &o, |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out2.status, FileStatus::Verified);
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert!(!tmp.exists());
        assert!(!journal.exists());
    }

    #[test]
    fn resume_aborts_when_source_changed() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("big.bin");
        fs::write(&src, vec![1u8; 400_000]).unwrap();
        let dst = t.path().join("out.bin");
        let mut o = opts();
        o.resume = true;
        let cancel = AtomicBool::new(false);
        let mut written = 0u64;
        let out = copy_file(
            &src,
            &dst,
            &o,
            |n| {
                written += n;
                if written >= 128 * 1024 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(out.status, FileStatus::Cancelled);
        // 源被修改（大小+1）
        let new_data = vec![2u8; 400_001];
        fs::write(&src, &new_data).unwrap();
        let out2 = copy_file(&src, &dst, &o, |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(out2.status, FileStatus::Verified);
        assert_eq!(fs::read(&dst).unwrap(), new_data);
    }

    #[test]
    fn resume_disabled_cleans_tmp_on_cancel() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("big.bin");
        fs::write(&src, vec![3u8; 400_000]).unwrap();
        let dst = t.path().join("out.bin");
        let mut o = opts();
        o.resume = false;
        let cancel = AtomicBool::new(false);
        let mut written = 0u64;
        let out = copy_file(
            &src,
            &dst,
            &o,
            |n| {
                written += n;
                if written >= 128 * 1024 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(out.status, FileStatus::Cancelled);
        assert!(!t.path().join(format!("out.bin{TMP_SUFFIX}")).exists());
        assert!(!t.path().join(format!("out.bin{RESUME_SUFFIX}")).exists());
    }
}
