use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::core::hash::HashAlgo;
use crate::core::verify::{parse_manifest, verify_against_manifest, VerifyItem};

pub enum JobKind {
    Manifest,
    Verify,
}

#[derive(Default)]
pub struct JobShared {
    pub progress: Option<(u32, u32)>,
    pub finished: bool,
    pub error: Option<String>,
    pub manifest_written: Option<String>,
    pub verify_result: Option<VerifyOutcome>,
}

pub struct VerifyOutcome {
    pub items: Vec<VerifyItem>,
    pub algo: HashAlgo,
    pub manifest_path: String,
    pub base: String,
}

pub struct JobHandle {
    pub kind: JobKind,
    pub shared: Arc<Mutex<JobShared>>,
    pub label: String,
}

pub fn start_manifest_build(root: PathBuf, algo: HashAlgo, save_to: PathBuf) -> JobHandle {
    let shared = Arc::new(Mutex::new(JobShared::default()));
    let label = format!("生成清单：{} → {}", root.display(), save_to.display());
    let shared_t = Arc::clone(&shared);
    std::thread::spawn(move || {
        let result = crate::core::verify::build_manifest(&root, algo)
            .map_err(|e| format!("生成清单失败：{e}"));
        match result {
            Ok(entries) => {
                let n = entries.len() as u32;
                if let Ok(mut g) = shared_t.lock() {
                    g.progress = Some((n, n));
                }
                match crate::core::verify::write_manifest(&save_to, &entries) {
                    Ok(_) => {
                        if let Ok(mut g) = shared_t.lock() {
                            g.manifest_written = Some(save_to.display().to_string());
                            g.finished = true;
                        }
                    }
                    Err(e) => {
                        if let Ok(mut g) = shared_t.lock() {
                            g.error = Some(format!("写入清单失败：{e}"));
                            g.finished = true;
                        }
                    }
                }
            }
            Err(e) => {
                if let Ok(mut g) = shared_t.lock() {
                    g.error = Some(e);
                    g.finished = true;
                }
            }
        }
    });
    JobHandle {
        kind: JobKind::Manifest,
        shared,
        label,
    }
}

pub fn start_verify(manifest_path: PathBuf, base: PathBuf, algo: HashAlgo) -> JobHandle {
    let shared = Arc::new(Mutex::new(JobShared::default()));
    let label = format!(
        "校验：{}（基目录 {}）",
        manifest_path.display(),
        base.display()
    );
    let shared_t = Arc::clone(&shared);
    std::thread::spawn(move || {
        let read = std::fs::read_to_string(crate::core::paths::to_long_path(&manifest_path))
            .map_err(|e| format!("读取清单失败：{e}"));
        match read {
            Ok(text) => {
                let entries = parse_manifest(&text);
                if entries.is_empty() {
                    if let Ok(mut g) = shared_t.lock() {
                        g.error = Some("清单为空或无法解析".into());
                        g.finished = true;
                    }
                    return;
                }
                let items = verify_against_manifest(&entries, &base, algo, |done, total| {
                    if let Ok(mut g) = shared_t.lock() {
                        g.progress = Some((done as u32, total as u32));
                    }
                });
                if let Ok(mut g) = shared_t.lock() {
                    g.verify_result = Some(VerifyOutcome {
                        items,
                        algo,
                        manifest_path: manifest_path.display().to_string(),
                        base: base.display().to_string(),
                    });
                    g.finished = true;
                }
            }
            Err(e) => {
                if let Ok(mut g) = shared_t.lock() {
                    g.error = Some(e);
                    g.finished = true;
                }
            }
        }
    });
    JobHandle {
        kind: JobKind::Verify,
        shared,
        label,
    }
}

pub struct JobSnapshot {
    pub progress: Option<(u32, u32)>,
    pub finished: bool,
}

pub fn poll(handle: &JobHandle) -> JobSnapshot {
    match handle.shared.lock() {
        Ok(g) => JobSnapshot {
            progress: g.progress,
            finished: g.finished,
        },
        Err(_) => JobSnapshot {
            progress: None,
            finished: false,
        },
    }
}

pub fn take_result(handle: &JobHandle) -> (Option<String>, Option<VerifyOutcome>, Option<String>) {
    match handle.shared.lock() {
        Ok(mut g) => {
            let v = g.verify_result.take();
            let m = g.manifest_written.take();
            let e = g.error.take();
            (m, v, e)
        }
        Err(_) => (None, None, None),
    }
}
