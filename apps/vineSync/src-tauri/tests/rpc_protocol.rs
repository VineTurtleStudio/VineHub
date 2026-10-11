//! 协议级集成测试：spawn 真实 `vinesync --rpc` exe。
//! 覆盖：握手 / sync.preview / sync.start 全流程 / 错误码。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;

struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl Client {
    fn spawn() -> Self {
        // 隔离数据目录：state.db / config.json 写到临时目录，不触碰真实 %APPDATA%
        let data_dir = temp_dir("data");
        let mut child = Command::new(env!("CARGO_BIN_EXE_vinesync"))
            .arg("--rpc")
            .env("VINESYNC_DATA_DIR", &data_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vinesync --rpc");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, stdin, rx }
    }

    fn send(&mut self, value: &Value) {
        let line = serde_json::to_string(value).unwrap();
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, id: i64, method: &str, params: Option<Value>) {
        let mut req = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            req["params"] = p;
        }
        self.send(&req);
    }

    fn recv(&self, timeout: Duration) -> Option<Value> {
        self.rx.recv_timeout(timeout).ok().and_then(|l| serde_json::from_str(&l).ok())
    }

    fn recv_response_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == Value::from(id) && msg.get("method").is_none() {
                return Some(msg);
            }
        }
        None
    }

    /// 终态 = 同 id 且带业务字段（非仅 taskId 的 ack）。
    fn recv_terminal_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == Value::from(id) && msg.get("method").is_none() {
                let r = &msg["result"];
                let is_terminal = r.get("counts").is_some()
                    || r.get("changeCount").is_some()
                    || r.get("totalChanges").is_some();
                if is_terminal || msg.get("error").is_some() {
                    return Some(msg);
                }
            }
        }
        None
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vinesync-rpc-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn handshake_reports_id_and_capabilities() {
    let mut client = Client::spawn();
    client.request(0, "plugin.info", None);
    let resp = client
        .recv_response_for(0, Duration::from_secs(5))
        .expect("plugin.info 应有响应");
    assert_eq!(resp["result"]["id"], "vine.sync");
    assert_eq!(resp["result"]["apiVersion"], "1.0");
    let caps: Vec<String> = resp["result"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(caps.contains(&"preview".to_string()));
    assert!(caps.contains(&"sync".to_string()));
}

#[test]
fn preview_then_sync_start_executes() {
    let src = temp_dir("src");
    let dst = temp_dir("dst");
    std::fs::write(src.join("a.txt"), "AAAA").unwrap();
    std::fs::write(src.join("b.txt"), "BBBB").unwrap();

    let mut client = Client::spawn();

    // preview：试算 2 项新增，不落盘
    client.request(
        1,
        "sync.preview",
        Some(serde_json::json!({
            "source": src.display().to_string(),
            "dest": dst.display().to_string(),
        })),
    );
    let ack = client
        .recv_response_for(1, Duration::from_secs(5))
        .expect("preview ack");
    assert!(ack["result"]["taskId"].is_string());
    let terminal = client
        .recv_terminal_for(1, Duration::from_secs(30))
        .expect("preview 终态");
    let counts = &terminal["result"]["counts"];
    assert_eq!(counts["added"], 2, "preview 应算出 2 项新增: {terminal}");
    assert!(!dst.join("a.txt").exists(), "preview 不应落盘");

    // sync.start：执行同步
    client.request(
        2,
        "sync.start",
        Some(serde_json::json!({
            "source": src.display().to_string(),
            "dest": dst.display().to_string(),
        })),
    );
    let ack = client
        .recv_response_for(2, Duration::from_secs(5))
        .expect("sync ack");
    assert!(ack["result"]["taskId"].is_string());
    let terminal = client
        .recv_terminal_for(2, Duration::from_secs(60))
        .expect("sync 终态");
    let result = &terminal["result"];
    assert_eq!(result["ok"], true, "sync 应成功: {result}");
    assert_eq!(result["counts"]["added"], 2);
    assert!(dst.join("a.txt").is_file(), "目标应有 a.txt");
    assert_eq!(std::fs::read_to_string(dst.join("b.txt")).unwrap(), "BBBB");

    // 再次 preview：0 变更
    client.request(
        3,
        "sync.preview",
        Some(serde_json::json!({
            "source": src.display().to_string(),
            "dest": dst.display().to_string(),
        })),
    );
    let terminal = client
        .recv_terminal_for(3, Duration::from_secs(30))
        .expect("第二次 preview 终态");
    assert_eq!(terminal["result"]["changeCount"], 0, "二次 preview 应无变更");

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

#[test]
fn invalid_params_rejected() {
    let mut client = Client::spawn();

    // 缺 source/dest/pairId → -32602
    client.request(10, "sync.preview", Some(serde_json::json!({})));
    let resp = client
        .recv_response_for(10, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 未知 pairId → -32602
    client.request(
        11,
        "sync.start",
        Some(serde_json::json!({ "pairId": "no-such-pair" })),
    );
    let resp = client
        .recv_response_for(11, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 未知方法 → -32601
    client.request(12, "nope", None);
    let resp = client
        .recv_response_for(12, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn state_get_filter_test_app_info_roundtrip() {
    let mut client = Client::spawn();

    // state.get：headless 状态（空 pair 列表 + 默认配置）
    client.request(20, "state.get", None);
    let resp = client
        .recv_response_for(20, Duration::from_secs(5))
        .expect("state.get 应有响应");
    let result = &resp["result"];
    assert!(result["config"].is_object(), "应含 config: {result}");
    assert_eq!(result["syncing"], false);
    assert!(result["pairStates"].as_array().unwrap().is_empty());

    // filter.test：命中标记
    client.request(
        21,
        "filter.test",
        Some(serde_json::json!({
            "include": ["**/*.txt"],
            "exclude": [],
            "path": "docs/readme.txt",
        })),
    );
    let resp = client
        .recv_response_for(21, Duration::from_secs(5))
        .expect("filter.test 应有响应");
    assert_eq!(resp["result"]["matched"], true, "得到 {resp}");

    // filter.test：排除命中
    client.request(
        22,
        "filter.test",
        Some(serde_json::json!({
            "include": ["**/*"],
            "exclude": ["*.tmp"],
            "path": "a/b.tmp",
        })),
    );
    let resp = client
        .recv_response_for(22, Duration::from_secs(5))
        .expect("filter.test 应有响应");
    assert_eq!(resp["result"]["matched"], false, "得到 {resp}");

    // app.info
    client.request(23, "app.info", None);
    let resp = client
        .recv_response_for(23, Duration::from_secs(5))
        .expect("app.info 应有响应");
    assert!(resp["result"]["dataDir"].is_string());
    assert!(resp["result"]["version"].is_string());
}

#[test]
fn config_save_history_roundtrip() {
    let mut client = Client::spawn();

    // 保存非法配置（copyThreads 越界）→ -32602
    client.request(
        30,
        "config.save",
        Some(serde_json::json!({
            "config": { "copyThreads": 99, "pairs": [] }
        })),
    );
    let resp = client
        .recv_response_for(30, Duration::from_secs(5))
        .expect("非法配置应报错");
    assert_eq!(resp["error"]["code"], -32602, "得到 {resp}");

    // 合法配置保存 → 回读一致（源/目标须互不包含）
    let pair_src = temp_dir("cfg-src");
    let pair_dst = temp_dir("cfg-dst");
    client.request(
        31,
        "config.save",
        Some(serde_json::json!({
            "config": {
                "copyThreads": 4,
                "pairs": [{
                    "id": "p1",
                    "name": "测试对",
                    "source": pair_src.display().to_string(),
                    "dest": pair_dst.display().to_string(),
                    "enabled": true,
                    "include": [],
                    "exclude": [],
                    "intervalMinutes": null
                }]
            }
        })),
    );
    let resp = client
        .recv_response_for(31, Duration::from_secs(5))
        .expect("保存配置应成功");
    assert_eq!(resp["result"]["config"]["copyThreads"], 4);
    assert_eq!(resp["result"]["config"]["pairs"][0]["id"], "p1", "得到 {resp}");

    // history.list：新库为空数组
    client.request(
        32,
        "history.list",
        Some(serde_json::json!({ "limit": 10 })),
    );
    let resp = client
        .recv_response_for(32, Duration::from_secs(5))
        .expect("history.list 应有响应");
    assert!(resp["result"]["rows"].as_array().unwrap().is_empty(), "得到 {resp}");

    // history.clear：ok
    client.request(33, "history.clear", None);
    let resp = client
        .recv_response_for(33, Duration::from_secs(5))
        .expect("history.clear 应有响应");
    assert_eq!(resp["result"]["ok"], true);
}
