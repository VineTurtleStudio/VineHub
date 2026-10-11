//! 协议级集成测试：spawn 真实 `vine-pulse --rpc` exe，走
//! 握手 → 扫描 → 进度通知 → 取消 → 终态全流程。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;
use zip::write::SimpleFileOptions;

/// 协议客户端：后台线程读 stdout 行，主线程带超时收消息。
struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl Client {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vine-pulse"))
            .arg("--rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vine-pulse --rpc");

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

    fn notify(&mut self, method: &str, params: Option<Value>) {
        let mut req = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        if let Some(p) = params {
            req["params"] = p;
        }
        self.send(&req);
    }

    /// 收一条消息；超时返回 None。
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
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-rpc-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 生成一个合法 zip。
fn write_zip(path: &Path) {
    use std::io::Write as _;
    let file = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("data.txt", opts).unwrap();
    w.write_all(b"hello vine").unwrap();
    w.finish().unwrap();
}

/// 生成一个损坏 zip（合法 PK 头 + 垃圾内容）。
fn write_bad_zip(path: &Path) {
    std::fs::write(path, b"PK\x03\x04 this is not a valid zip central directory ~~~~~").unwrap();
}

#[test]
fn handshake_returns_plugin_info() {
    let mut client = Client::spawn();
    client.request(0, "plugin.info", None);
    let resp = client
        .recv_response_for(0, Duration::from_secs(5))
        .expect("应收到 plugin.info 响应");

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["result"]["id"], "vine.pulse");
    assert_eq!(resp["result"]["apiVersion"], "1.0");
    let caps = resp["result"]["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "scan"));
    assert!(caps.iter().any(|c| c == "report"));
}

#[test]
fn unknown_method_returns_method_not_found() {
    let mut client = Client::spawn();
    client.request(7, "nope.thing", None);
    let resp = client
        .recv_response_for(7, Duration::from_secs(5))
        .expect("应收到错误响应");
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn scan_start_progress_and_terminal() {
    let dir = temp_dir("scan");
    write_zip(&dir.join("good.zip"));
    write_bad_zip(&dir.join("bad.zip"));
    std::fs::write(dir.join("note.txt"), b"plain text").unwrap();

    let mut client = Client::spawn();
    client.request(
        1,
        "scan.start",
        Some(serde_json::json!({ "path": dir.to_string_lossy() })),
    );

    // ack：立即回 taskId
    let ack = client
        .recv_response_for(1, Duration::from_secs(5))
        .expect("应收到 ack");
    let task_id = ack["result"]["taskId"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("t-"));

    // 进度通知 + 终态（同 id 第二响应）
    let mut progress_seen = 0u32;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let terminal = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "等待终态超时（已见 {progress_seen} 条进度）"
        );
        let msg = client
            .recv(Duration::from_secs(10))
            .expect("连接不应断开");
        if msg.get("method").is_some() {
            assert_eq!(msg["method"], "task.progress");
            assert_eq!(msg["params"]["taskId"], task_id.as_str());
            progress_seen += 1;
            continue;
        }
        if msg["id"] == Value::from(1) {
            break msg;
        }
    };

    assert!(progress_seen >= 1, "应至少收到一条进度通知");
    let result = &terminal["result"];
    assert_eq!(result["taskId"], task_id.as_str());
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["discovered"], 3);
    assert_eq!(result["scanned"], 3);
    // bad.zip 应被判为有问题；good.zip / note.txt 正常
    assert_eq!(result["ok"], 2);
    assert_eq!(result["bad"], 1);

    // 报告内容可供 hub 结果表渲染
    let reports = result["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 3);
    let bad_report = reports
        .iter()
        .find(|r| r["path"].as_str().unwrap().contains("bad.zip"))
        .unwrap();
    let worst_rank = bad_report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| match f["severity"].as_str().unwrap_or("ok") {
            "ok" => 0,
            "warning" => 1,
            "unknown" => 2,
            "corrupt" => 3,
            other => panic!("未知 severity: {other}"),
        })
        .max()
        .unwrap();
    assert!(worst_rank > 0, "bad.zip 应有问题 findings: {bad_report}");

    // task.cancel 对已结束任务 → TASK_NOT_FOUND
    client.request(
        2,
        "task.cancel",
        Some(serde_json::json!({ "taskId": task_id })),
    );
    let resp = client
        .recv_response_for(2, Duration::from_secs(5))
        .expect("应收到取消响应");
    assert_eq!(resp["error"]["code"], -32004);
}

#[test]
fn scan_cancel_yields_cancelled_terminal() {
    let dir = temp_dir("cancel");
    // 随机噪声 PNG 解码较慢，保证取消能落在扫描中途（每张约几十毫秒）
    use image::{Rgb, RgbImage};
    let mut rng_state = 0x2545F491u32;
    for i in 0..30 {
        let img = RgbImage::from_fn(1024, 1024, |_, _| {
            // xorshift 伪随机像素
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 17;
            rng_state ^= rng_state << 5;
            Rgb([(rng_state >> 16) as u8, (rng_state >> 8) as u8, rng_state as u8])
        });
        img.save(dir.join(format!("img{i:02}.png"))).unwrap();
    }

    let mut client = Client::spawn();
    client.request(
        1,
        "scan.start",
        Some(serde_json::json!({ "path": dir.to_string_lossy() })),
    );
    let ack = client
        .recv_response_for(1, Duration::from_secs(5))
        .expect("应收到 ack");
    let task_id = ack["result"]["taskId"].as_str().unwrap().to_string();

    // 收到第一条进度即取消（此时大多数 PNG 仍在解码）
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut got_progress = false;
    while !got_progress {
        assert!(std::time::Instant::now() < deadline, "等待进度超时");
        let msg = client.recv(Duration::from_secs(10)).expect("连接不应断开");
        if msg.get("method").is_some() {
            assert_eq!(msg["method"], "task.progress");
            got_progress = true;
        }
    }
    client.notify(
        "task.cancel",
        Some(serde_json::json!({ "taskId": task_id })),
    );

    // 终态：cancelled=true，且未扫完全部（仍有文件未派发）
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let terminal = loop {
        assert!(std::time::Instant::now() < deadline, "等待取消终态超时");
        let msg = client.recv(Duration::from_secs(15)).expect("连接不应断开");
        if msg.get("method").is_none() && msg["id"] == Value::from(1) {
            break msg;
        }
    };
    assert_eq!(terminal["result"]["cancelled"], true);
    let scanned = terminal["result"]["scanned"].as_u64().unwrap();
    assert!(
        scanned < 30,
        "取消后不应扫完全部，实际 {scanned}"
    );
}

#[test]
fn scan_rejects_missing_path_and_bad_params() {
    let mut client = Client::spawn();

    client.request(1, "scan.start", Some(serde_json::json!({ "path": "Z:\\no\\such\\dir" })));
    let resp = client.recv_response_for(1, Duration::from_secs(5)).unwrap();
    assert_eq!(resp["error"]["code"], -32602);

    client.request(2, "scan.start", Some(serde_json::json!({})));
    let resp = client.recv_response_for(2, Duration::from_secs(5)).unwrap();
    assert_eq!(resp["error"]["code"], -32602);

    client.request(
        3,
        "task.cancel",
        Some(serde_json::json!({ "taskId": "t-none" })),
    );
    let resp = client.recv_response_for(3, Duration::from_secs(5)).unwrap();
    assert_eq!(resp["error"]["code"], -32004);
}

#[test]
fn type_filter_limits_targets() {
    let dir = temp_dir("filter");
    write_zip(&dir.join("a.zip"));
    write_zip(&dir.join("b.png.zip")); // 扩展名仍是 zip
    std::fs::write(dir.join("c.txt"), b"x").unwrap();

    let mut client = Client::spawn();
    client.request(
        1,
        "scan.start",
        Some(serde_json::json!({ "path": dir.to_string_lossy(), "types": ["zip"] })),
    );
    let ack = client.recv_response_for(1, Duration::from_secs(5)).unwrap();
    let task_id = ack["result"]["taskId"].as_str().unwrap().to_string();

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let terminal = loop {
        assert!(std::time::Instant::now() < deadline, "等待终态超时");
        let msg = client.recv(Duration::from_secs(10)).expect("连接不应断开");
        if msg.get("method").is_none() && msg["result"]["taskId"] == task_id.as_str() {
            break msg;
        }
    };
    assert_eq!(terminal["result"]["discovered"], 2);
    assert_eq!(terminal["result"]["ok"], 2);
}
