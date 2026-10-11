use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::dispatch;
use super::{Opts, Report};

/// 收集待扫描文件（递归展开目录，跳过符号链接以避免循环）
///
/// 返回 (文件列表, 错误描述列表)
pub fn collect_files(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    let mut queue: VecDeque<PathBuf> = paths.iter().cloned().collect();

    while let Some(path) = queue.pop_front() {
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                errors.push(format!("{}：{e}", path.display()));
                continue;
            }
        };
        let ft = meta.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_file() {
            files.push(path);
        } else if ft.is_dir() {
            match std::fs::read_dir(&path) {
                Ok(rd) => {
                    for entry in rd.flatten() {
                        queue.push_back(entry.path());
                    }
                }
                Err(e) => errors.push(format!("{}：{e}", path.display())),
            }
        }
    }
    (files, errors)
}

/// 并行扫描一批文件。
///
/// `progress(done, total)` 在每个文件完成后调用；`cancel` 置真后停止派发新任务。
pub fn scan<F>(files: &[PathBuf], opts: &Opts, cancel: &AtomicBool, progress: F) -> Vec<Report>
where
    F: Fn(usize, usize) + Sync,
{
    let total = files.len();
    if total == 0 {
        return Vec::new();
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(total)
        .max(1);

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<Report>> = Mutex::new(Vec::with_capacity(total));

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= total || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let report = dispatch::verify_file(&files[i], opts);
                    let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                    progress(d, total);
                    results.lock().expect("results lock").push(report);
                }
            });
        }
    });

    let mut reports = results.into_inner().expect("results lock");
    reports.sort_by(|a, b| a.path.cmp(&b.path));
    reports
}
