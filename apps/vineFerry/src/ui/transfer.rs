use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::conflict::ConflictPolicy;
use crate::core::copy::{run_transfer, Progress, RunSummary, TransferMode, TransferOptions};
use crate::core::plan::{available_space, build_plan, concat, PlanError};

#[derive(Default)]
pub struct TransferShared {
    /// 计划仍在后台扫描（build_plan/空间预检），尚未开始搬数据。
    pub scanning: bool,
    pub label: String,
    pub progress: Option<Progress>,
    pub summary: Option<RunSummary>,
    pub error: Option<String>,
    pub finished: bool,
}

pub struct TransferHandle {
    pub shared: Arc<Mutex<TransferShared>>,
    pub cancel: Arc<AtomicBool>,
    /// 预扫描前的简短标签（扫描完成后以 `TransferState::label` 为准）。
    pub label: String,
}

#[derive(Clone)]
pub struct TransferState {
    pub scanning: bool,
    pub label: String,
    pub progress: Option<Progress>,
    pub summary: Option<RunSummary>,
    pub error: Option<String>,
    pub finished: bool,
}

pub fn start_transfer(
    items: Vec<std::path::PathBuf>,
    dest: std::path::PathBuf,
    mode: TransferMode,
) -> Result<TransferHandle, String> {
    if items.is_empty() {
        return Err("没有选中任何文件".into());
    }
    if !dest.is_dir() {
        return Err(format!("目标目录不存在：{}", dest.display()));
    }

    let brief = format!(
        "{} → {}",
        items
            .first()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default(),
        dest.display()
    );

    let shared = Arc::new(Mutex::new(TransferShared {
        scanning: true,
        label: brief.clone(),
        ..Default::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let opts = TransferOptions {
        mode,
        ..Default::default()
    };

    let shared_t = Arc::clone(&shared);
    let cancel_t = Arc::clone(&cancel);
    // 计划构建（WalkDir + 逐文件 exists）整体挪进工作线程，UI 不再阻塞。
    std::thread::spawn(move || {
        fn fail(shared: &Mutex<TransferShared>, msg: String) {
            if let Ok(mut g) = shared.lock() {
                g.error = Some(msg);
                g.scanning = false;
                g.finished = true;
            }
        }
        let mut plans = Vec::new();
        for item in &items {
            if cancel_t.load(Ordering::Relaxed) {
                if let Ok(mut g) = shared_t.lock() {
                    g.scanning = false;
                    g.finished = true;
                }
                return;
            }
            match build_plan(item, &dest, ConflictPolicy::Rename) {
                Ok(p) => plans.push(p),
                Err(PlanError::UnresolvedConflicts(_)) => continue,
                Err(e) => {
                    fail(&shared_t, e.to_string());
                    return;
                }
            }
        }
        let plan = concat(plans);
        if plan.items.is_empty() {
            fail(&shared_t, "计划为空（没有可转移的文件）".into());
            return;
        }
        if let Ok(free) = available_space(&dest)
            && plan.transfer_bytes > free
        {
            fail(
                &shared_t,
                format!(
                    "目标空间不足：需要 {}，可用 {}",
                    crate::ui::pane::format_size(plan.transfer_bytes),
                    crate::ui::pane::format_size(free)
                ),
            );
            return;
        }

        {
            if let Ok(mut g) = shared_t.lock() {
                g.label = format!(
                    "{} → {}（{} 个文件，{}）",
                    items
                        .first()
                        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
                        .unwrap_or_default(),
                    dest.display(),
                    plan.items.len(),
                    crate::ui::pane::format_size(plan.transfer_bytes)
                );
                g.scanning = false;
            }
        }
        if cancel_t.load(Ordering::Relaxed) {
            if let Ok(mut g) = shared_t.lock() {
                g.finished = true;
            }
            return;
        }

        let summary = run_transfer(
            &plan,
            &opts,
            |p| {
                if let Ok(mut g) = shared_t.lock() {
                    g.progress = Some(p.clone());
                }
            },
            &cancel_t,
        );
        if let Ok(mut g) = shared_t.lock() {
            g.summary = Some(summary);
            g.finished = true;
        }
    });

    Ok(TransferHandle {
        shared,
        cancel,
        label: brief,
    })
}

pub fn poll(handle: &TransferHandle) -> TransferState {
    match handle.shared.lock() {
        Ok(g) => TransferState {
            scanning: g.scanning,
            label: if g.label.is_empty() {
                handle.label.clone()
            } else {
                g.label.clone()
            },
            progress: g.progress.clone(),
            summary: g.summary.clone(),
            error: g.error.clone(),
            finished: g.finished,
        },
        Err(_) => TransferState {
            scanning: false,
            label: handle.label.clone(),
            progress: None,
            summary: None,
            error: None,
            finished: false,
        },
    }
}
