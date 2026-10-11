use crate::model::TriggerMode;
use crate::runtime::request_sync;
use crate::AppCtx;
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// 实时监控状态（由 notify 回调写入事件，调度线程消费）
#[derive(Default)]
pub struct WatchState {
    pub watcher: Mutex<Option<notify::RecommendedWatcher>>,
    pub sources: Mutex<Vec<String>>,
    pub events: Mutex<Vec<PathBuf>>,
}

/// 定时与实时触发状态
#[derive(Default)]
pub struct TriggerState {
    pub last_interval: Mutex<Option<Instant>>,
    pub last_events: Mutex<HashMap<String, Instant>>,
    /// 各文件夹对的定时覆盖时钟（`interval_minutes` 覆盖生效时使用）
    pub last_per_pair: Mutex<HashMap<String, Instant>>,
}

/// 调度线程：每 500ms 检查一次定时/实时触发条件
pub fn start_trigger_thread(app: AppHandle) {
    std::thread::spawn(move || {
        let mut ticks: u64 = 0;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            ticks += 1;

            let (mode, interval_min, debounce, pairs) = {
                let ctx = app.state::<AppCtx>();
                let cfg = ctx.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let pairs: Vec<(String, String, Option<u32>)> = cfg
                    .pairs
                    .iter()
                    .filter(|p| p.enabled)
                    .map(|p| (p.id.clone(), p.source.clone(), p.interval_minutes))
                    .collect();
                (
                    cfg.trigger.mode,
                    cfg.trigger.interval_minutes.max(1),
                    cfg.trigger.debounce_ms.max(200),
                    pairs,
                )
            };

            if ticks % 60 == 0 {
                refresh_watchers(&app);
            }

            match mode {
                TriggerMode::Manual => {
                    let ctx = app.state::<AppCtx>();
                    let mut slot = ctx.trigger.last_interval.lock().unwrap_or_else(|e| e.into_inner());
                    *slot = None;
                    ctx.trigger.last_per_pair.lock().unwrap_or_else(|e| e.into_inner()).clear();
                }
                TriggerMode::Interval => {
                    // 逐个文件夹对判断：有间隔覆盖的用独立时钟，否则共享全局时钟
                    let due: Vec<String> = {
                        let ctx = app.state::<AppCtx>();
                        let now = Instant::now();
                        let mut slot = ctx.trigger.last_interval.lock().unwrap_or_else(|e| e.into_inner());
                        let mut per_pair = ctx.trigger
                            .last_per_pair
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let mut due = Vec::new();
                        for (id, _, override_min) in &pairs {
                            let minutes = override_min.unwrap_or(interval_min).max(1) as u64;
                            let last = if override_min.is_some() {
                                per_pair.get(id).copied()
                            } else {
                                *slot
                            };
                            match last {
                                Some(prev)
                                    if now.duration_since(prev)
                                        >= Duration::from_secs(minutes * 60) =>
                                {
                                    due.push(id.clone());
                                    if override_min.is_some() {
                                        per_pair.insert(id.clone(), now);
                                    } else {
                                        *slot = Some(now);
                                    }
                                }
                                None => {
                                    if override_min.is_some() {
                                        per_pair.insert(id.clone(), now);
                                    } else {
                                        *slot = Some(now);
                                    }
                                }
                                _ => {}
                            }
                        }
                        // 清理已删除文件夹对的时钟
                        per_pair.retain(|k, _| pairs.iter().any(|(id, _, _)| id == k));
                        due
                    };
                    if !due.is_empty() {
                        request_sync(&app, due, "interval");
                    }
                }
                TriggerMode::Realtime => {
                    // 消费文件事件并刷新各文件夹对的「最近变动」时间
                    let evs: Vec<PathBuf> = {
                        let ctx = app.state::<AppCtx>();
                        let mut w = ctx.watch.events.lock().unwrap_or_else(|e| e.into_inner());
                        std::mem::take(&mut *w)
                    };
                    {
                        let ctx = app.state::<AppCtx>();
                        let mut slot = ctx.trigger.last_interval.lock().unwrap_or_else(|e| e.into_inner());
                        *slot = None;
                        ctx.trigger.last_per_pair.lock().unwrap_or_else(|e| e.into_inner()).clear();
                    }
                    if !evs.is_empty() {
                        let ctx = app.state::<AppCtx>();
                        let mut le = ctx.trigger.last_events.lock().unwrap_or_else(|e| e.into_inner());
                        for (id, source, _) in &pairs {
                            let src = PathBuf::from(source);
                            if evs.iter().any(|e| e.starts_with(&src)) {
                                le.insert(id.clone(), Instant::now());
                            }
                        }
                    }
                    // 静默时间达到防抖阈值 → 触发
                    let due: Vec<String> = {
                        let ctx = app.state::<AppCtx>();
                        let mut le = ctx.trigger.last_events.lock().unwrap_or_else(|e| e.into_inner());
                        let now = Instant::now();
                        let mut due = Vec::new();
                        for (id, _, _) in &pairs {
                            if let Some(t) = le.get(id) {
                                if now.duration_since(*t) >= Duration::from_millis(debounce) {
                                    due.push(id.clone());
                                }
                            }
                        }
                        for id in &due {
                            le.remove(id);
                        }
                        due
                    };
                    if !due.is_empty() {
                        request_sync(&app, due, "realtime");
                    }
                }
            }
        }
    });
}

/// 按当前配置重建实时监控（仅在监听集合发生变化时重建）
pub fn refresh_watchers(app: &AppHandle) {
    let ctx = app.state::<AppCtx>();
    let desired: Vec<String> = {
        let cfg = ctx.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if cfg.trigger.mode == TriggerMode::Realtime {
            cfg.pairs
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.source.clone())
                .collect()
        } else {
            Vec::new()
        }
    };

    let current = ctx.watch.sources.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if desired == current {
        return;
    }

    let mut watcher_opt = None;
    let mut watched: Vec<String> = Vec::new();

    if !desired.is_empty() {
        let app2 = app.clone();
        let result = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let ctx = app2.state::<AppCtx>();
                let mut q = ctx.watch.events.lock().unwrap_or_else(|e| e.into_inner());
                for p in ev.paths {
                    q.push(p);
                }
                if q.len() > 2000 {
                    let n = q.len() - 2000;
                    q.drain(0..n);
                }
            }
        });
        match result {
            Ok(mut w) => {
                for src in &desired {
                    let path = Path::new(src);
                    if path.is_dir() && w.watch(path, RecursiveMode::Recursive).is_ok() {
                        watched.push(src.clone());
                    }
                }
                watcher_opt = Some(w);
            }
            Err(e) => eprintln!("{}", crate::msg!("初始化文件监控失败: {e}", "Failed to init file watcher: {e}")),
        }
    }

    *ctx.watch.watcher.lock().unwrap_or_else(|e| e.into_inner()) = watcher_opt;
    *ctx.watch.sources.lock().unwrap_or_else(|e| e.into_inner()) = watched;
    // 若 watched != desired（部分源监听失败），下次刷新会自动重试
}
