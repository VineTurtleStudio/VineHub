//! 协议级集成测试：spawn 真实 `vine-press --rpc` exe。
//! 覆盖：握手 / plan.start 全流程 / dupes.find / export.start(zip) / 错误码。

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
        let mut child = Command::new(env!("CARGO_BIN_EXE_vine-press"))
            .arg("--rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vine-press --rpc");

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

    /// 等待某 id 的终态响应（ack 之后的第二响应；ack 仅有 taskId，终态另带业务字段）。
    fn recv_terminal_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == Value::from(id) && msg.get("method").is_none() {
                let r = &msg["result"];
                let is_terminal = r.get("unitCount").is_some()
                    || r.get("groupCount").is_some()
                    || r.get("status").is_some();
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
        "vine-press-rpc-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 造一个最小可识别单位：目录 + 若干普通文件。
fn make_unit(root: &std::path::Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("file01.txt"), b"hello press").unwrap();
    std::fs::write(dir.join("file02.txt"), b"world press").unwrap();
}

#[test]
fn handshake_reports_id_and_capabilities() {
    let mut client = Client::spawn();
    client.request(0, "plugin.info", None);
    let resp = client
        .recv_response_for(0, Duration::from_secs(5))
        .expect("plugin.info 应有响应");
    let info = &resp["result"];
    assert_eq!(info["id"], "vine.press");
    assert_eq!(info["apiVersion"], "1.0");
    let caps: Vec<String> = info["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(caps.contains(&"plan".to_string()));
    assert!(caps.contains(&"export".to_string()));
}

#[test]
fn plan_start_ack_progress_terminal() {
    let root = temp_dir("plan");
    make_unit(&root, "Album01");
    make_unit(&root, "Album02");

    let mut client = Client::spawn();
    client.request(
        1,
        "plan.start",
        Some(serde_json::json!({ "root": root.display().to_string() })),
    );

    // ack：立即回 taskId
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut task_id = None;
    while std::time::Instant::now() < deadline && task_id.is_none() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(msg) = client.recv(remaining) else { break };
        if msg["id"] == Value::from(1) && msg.get("method").is_none() {
            task_id = msg["result"]["taskId"].as_str().map(|s| s.to_string());
        }
    }
    let task_id = task_id.expect("ack 应带回 taskId");

    // 终态：同 id 第二响应
    let terminal = client
        .recv_terminal_for(1, Duration::from_secs(15))
        .expect("plan 终态应送达");
    let result = &terminal["result"];
    assert_eq!(result["taskId"], task_id.as_str());
    assert_eq!(result["cancelled"], false);
    assert!(
        result["unitCount"].as_u64().unwrap_or(0) >= 2,
        "至少识别 2 个单位，得到 {}",
        result["unitCount"]
    );
    let units = result["units"].as_array().expect("units 数组");
    assert!(units.len() >= 2);
    assert!(units[0].get("relPath").is_some());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dupes_finds_identical_files() {
    let root = temp_dir("dupes");
    let a = root.join("a");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::write(a.join("copy1.bin"), b"same bytes here").unwrap();
    std::fs::write(a.join("copy2.bin"), b"same bytes here").unwrap();

    let mut client = Client::spawn();
    client.request(
        2,
        "dupes.find",
        Some(serde_json::json!({ "root": root.display().to_string() })),
    );

    let ack = client
        .recv_response_for(2, Duration::from_secs(5))
        .expect("dupes ack 应送达");
    assert!(ack["result"]["taskId"].is_string());

    let terminal = client
        .recv_terminal_for(2, Duration::from_secs(15))
        .expect("dupes 终态应送达");
    assert!(
        terminal["result"]["groupCount"].as_u64().unwrap_or(0) >= 1,
        "应找到 ≥1 组重复，得到 {}",
        terminal["result"]["groupCount"]
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn export_start_creates_zip() {
    let root = temp_dir("export-src");
    let out = temp_dir("export-out");
    make_unit(&root, "UnitA");

    let mut client = Client::spawn();
    client.request(
        3,
        "export.start",
        Some(serde_json::json!({
            "root": root.display().to_string(),
            "outputRoot": out.display().to_string(),
        })),
    );

    let ack = client
        .recv_response_for(3, Duration::from_secs(5))
        .expect("export ack 应送达");
    assert!(
        ack["result"]["taskId"].is_string(),
        "ack 应含 taskId，得到 {ack}"
    );

    let terminal = client
        .recv_terminal_for(3, Duration::from_secs(30))
        .expect("export 终态应送达");
    let result = &terminal["result"];
    assert_eq!(result["status"], "finished", "得到 {result}");
    assert!(result["created"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(result["failed"].as_u64().unwrap_or(0), 0);

    // 终态 archives 可能为空，以输出目录实际产物为准
    let zips: Vec<_> = walk_zips(&out);
    assert!(!zips.is_empty(), "输出目录应有 zip 产物: {}", out.display());

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out);
}

fn walk_zips(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("zip") {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn invalid_params_and_unknown_method() {
    let mut client = Client::spawn();

    // 缺 root → -32602
    client.request(10, "plan.start", Some(serde_json::json!({})));
    let resp = client
        .recv_response_for(10, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 未知方法 → -32601
    client.request(11, "nope", None);
    let resp = client
        .recv_response_for(11, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32601);

    // 非 zip 导出 → -32602
    client.request(
        12,
        "export.start",
        Some(serde_json::json!({
            "root": std::env::temp_dir().display().to_string(),
            "outputRoot": std::env::temp_dir().display().to_string(),
            "settings": { "format": "rar" }
        })),
    );
    let resp = client
        .recv_response_for(12, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);
}
