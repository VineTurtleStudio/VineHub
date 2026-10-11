//! 协议级集成测试：spawn 真实假插件 exe → 握手 → 请求 → 通知 → 超时 → 杀进程。

use std::time::{Duration, Instant};

use serde_json::json;
use vine_host_core::sidecar::{SidecarConfig, SidecarError, SidecarManager};
use vine_plugin_api::method;

fn fake_plugin_exe() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_vine_fake_plugin"))
}

#[test]
fn handshake_request_notification_kill() {
    let mut manager = SidecarManager::new();
    let config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    let handle = manager.spawn(config).expect("握手应成功");

    assert!(handle.is_running());
    assert_eq!(handle.info.id, "vine.fake");
    assert_eq!(handle.info.api_version, "1.0");

    // 请求 → 响应 id 配对
    let result = handle
        .request("echo", Some(json!({ "msg": "hello" })))
        .expect("echo 应成功");
    assert_eq!(result["msg"], "hello");

    // 进度通知 + 终态响应
    let result = handle
        .request("progress.demo", None)
        .expect("progress.demo 应返回终态");
    assert_eq!(result["taskId"], "t-demo");
    let mut progress_seen = 0;
    while let Some(note) = handle.try_recv_notification() {
        assert_eq!(note.method, method::TASK_PROGRESS);
        assert!(note.is_notification());
        progress_seen += 1;
    }
    assert_eq!(progress_seen, 2, "应收到 2 条 task.progress 通知");

    // 未知方法 → MethodNotFound
    let err = handle.request("nope", None).unwrap_err();
    assert!(
        matches!(err, SidecarError::Rpc { code, .. } if code == vine_plugin_api::codes::METHOD_NOT_FOUND),
        "得到 {err:?}"
    );

    // 杀进程树并回收
    let pid = {
        // 通过 is_running 间接验证；杀完再确认
        handle.kill();
        handle.is_running()
    };
    assert!(!pid, "kill 后子进程应已退出");

    // 杀后请求 → NotRunning
    let err = handle.request("echo", None).unwrap_err();
    assert!(matches!(err, SidecarError::NotRunning), "得到 {err:?}");
}

#[test]
fn request_timeout_does_not_kill_and_recovers() {
    let mut manager = SidecarManager::new();
    let mut config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    config.request_timeout = Duration::from_millis(300);
    let handle = manager.spawn(config).expect("握手应成功");

    let started = Instant::now();
    let err = handle.request("block", None).unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(err, SidecarError::Timeout { .. }), "得到 {err:?}");
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(3),
        "超时应在 300ms 附近触发，实际 {elapsed:?}"
    );

    // 超时后连接仍可用（block 线程被占不影响其他请求）
    let result = handle.request("echo", Some(json!({ "ok": 1 }))).unwrap();
    assert_eq!(result["ok"], 1);
}

#[test]
fn crash_fails_pending_and_surfaces_dead_process() {
    let mut manager = SidecarManager::new();
    let config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    let handle = manager.spawn(config).expect("握手应成功");

    // crash 请求使进程退出；挂在 pending 的请求应收到失败而非干等
    let crashed = handle.request("crash", None);
    assert!(crashed.is_err(), "crash 后请求应失败");

    // 等待进程真正退出（reader 线程 EOF 回收 pending 已发生）
    let deadline = Instant::now() + Duration::from_secs(3);
    while handle.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!handle.is_running(), "crash 后进程应退出");

    let err = handle.request("echo", None).unwrap_err();
    assert!(matches!(err, SidecarError::NotRunning), "得到 {err:?}");
}

#[test]
fn handshake_rejects_wrong_id() {
    // 直接用脚本行为模拟：期望 id 与夹具不符
    let mut manager = SidecarManager::new();
    let config = SidecarConfig::new("vine.wrong", fake_plugin_exe());
    let err = manager.spawn(config).unwrap_err();
    match err {
        SidecarError::Handshake(msg) => assert!(msg.contains("id 不匹配"), "得到 {msg}"),
        other => panic!("应为 Handshake 错误，得到 {other:?}"),
    }
    assert!(manager.get("vine.wrong").is_none(), "失败的 spawn 不应留在管理器");
}

#[test]
fn manager_spawn_replaces_existing_connection() {
    let mut manager = SidecarManager::new();
    let config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    let first = manager.spawn(config.clone()).unwrap();
    assert!(first.is_running());

    let second = manager.spawn(config).unwrap();
    assert!(second.is_running());
    // 旧句柄已被 manager 替换并 kill
    assert!(!first.is_running());
}

#[test]
fn request_with_ack_returns_ack_then_terminal() {
    let mut manager = SidecarManager::new();
    let config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    let handle = manager.spawn(config).expect("握手应成功");

    let (ack, terminal) = handle
        .request_with_ack(
            "scan.demo",
            None,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .expect("双响应请求应成功");

    assert_eq!(ack["taskId"], "t-scan-demo");
    assert_eq!(terminal["taskId"], "t-scan-demo");
    assert_eq!(terminal["ok"], 2);
    assert_eq!(terminal["bad"], 1);
    assert_eq!(terminal["cancelled"], false);

    // 进度通知应在终态前送达
    let mut progress = 0;
    while let Some(note) = handle.try_recv_notification() {
        assert_eq!(note.method, method::TASK_PROGRESS);
        progress += 1;
    }
    assert_eq!(progress, 3);
}

#[test]
fn request_with_ack_timeout_cleans_pending() {
    let mut manager = SidecarManager::new();
    let mut config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    config.request_timeout = Duration::from_secs(2);
    let handle = manager.spawn(config).expect("握手应成功");

    // block 永不回 ack → ack 超时
    let err = handle
        .request_with_ack("block", None, Duration::from_millis(300), Duration::from_secs(5))
        .unwrap_err();
    assert!(matches!(err, SidecarError::Timeout { .. }), "得到 {err:?}");

    // 连接仍可用
    let result = handle.request("echo", Some(json!({ "ok": 1 }))).unwrap();
    assert_eq!(result["ok"], 1);
}

#[test]
fn stderr_log_is_captured() {
    let mut manager = SidecarManager::new();
    // 夹具启动即写一行 stderr，通道应可捕获
    let config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    let handle = manager.spawn(config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let log = handle.take_stderr_log();
        if log.iter().any(|l| l.contains("started")) {
            break;
        }
        if Instant::now() > deadline {
            panic!("2s 内未捕获到 started 日志行: {log:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(handle.is_running());
}

#[test]
fn heartbeat_kills_hung_plugin() {
    let mut manager = SidecarManager::new();
    let mut config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    config.heartbeat_interval = Duration::from_millis(150);
    config.ping_timeout = Duration::from_millis(200);
    config.heartbeat_max_misses = 3;
    config.request_timeout = Duration::from_millis(200);
    let handle = manager.spawn(config).expect("握手应成功");
    assert!(handle.is_running());

    // hang 卡死主循环 → 后续 ping 无响应 → 心跳判定假死击杀
    let _ = handle.request("hang", None);

    let deadline = Instant::now() + Duration::from_secs(8);
    while handle.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!handle.is_running(), "假死插件应被心跳击杀");

    // 击杀后请求快速失败（不干等超时）
    let started = Instant::now();
    let err = handle.request("echo", None).unwrap_err();
    assert!(matches!(err, SidecarError::NotRunning), "得到 {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "击杀后请求应立即失败"
    );

    // 重拉可用（宿主重试路径）
    let mut config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    config.heartbeat_interval = Duration::ZERO;
    let fresh = manager.spawn(config).expect("重拉应成功");
    assert!(fresh.is_running());
}

#[test]
fn heartbeat_disabled_by_zero_interval() {
    let mut manager = SidecarManager::new();
    let mut config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    config.heartbeat_interval = Duration::ZERO;
    let handle = manager.spawn(config).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(handle.is_running(), "关闭心跳时不应有击杀动作");
}

#[test]
fn stderr_log_written_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SidecarManager::new();
    let mut config = SidecarConfig::new("vine.fake", fake_plugin_exe());
    config.heartbeat_interval = Duration::ZERO;
    config.log_dir = Some(dir.path().to_path_buf());
    let handle = manager.spawn(config).unwrap();

    let log_path = dir.path().join("vine.fake.log");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if log_path.is_file() {
            let content = std::fs::read_to_string(&log_path).unwrap_or_default();
            if content.contains("started") {
                break;
            }
        }
        if Instant::now() > deadline {
            panic!("2s 内日志文件未写入 started 行");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    handle.kill();
}
