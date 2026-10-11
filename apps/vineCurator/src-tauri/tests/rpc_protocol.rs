//! 协议级集成测试：spawn 真实 `vine-curator --rpc` exe（隔离临时 DB）。
//! 覆盖：握手 / addRoot→scan→search 全流程 / 错误码。

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
    fn spawn(db_path: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vine-curator"))
            .arg("--rpc")
            .env("VINECURATOR_DB", db_path.as_os_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vine-curator --rpc");

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

    fn recv_terminal_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == Value::from(id) && msg.get("method").is_none() {
                let r = &msg["result"];
                let is_terminal = r.get("stats").is_some() || r.get("status").is_some();
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

fn temp_paths(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "vine-curator-rpc-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    (base.join("catalog.sqlite"), base.join("library"))
}

#[test]
fn handshake_reports_id_and_capabilities() {
    let (db, _) = temp_paths("handshake");
    let mut client = Client::spawn(&db);
    client.request(0, "plugin.info", None);
    let resp = client
        .recv_response_for(0, Duration::from_secs(10))
        .expect("plugin.info 应有响应");
    assert_eq!(resp["result"]["id"], "vine.curator");
    assert_eq!(resp["result"]["apiVersion"], "1.0");
    let caps: Vec<String> = resp["result"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(caps.contains(&"library".to_string()));
    assert!(caps.contains(&"search".to_string()));
}

#[test]
fn add_root_scan_search_flow() {
    let (db, lib) = temp_paths("flow");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("SongA.mp3"), b"ID3 fake audio content").unwrap();
    std::fs::write(lib.join("curator-notes.txt"), b"plain text note").unwrap();

    let mut client = Client::spawn(&db);

    // addRoot
    client.request(
        1,
        "library.addRoot",
        Some(serde_json::json!({ "path": lib.display().to_string() })),
    );
    let resp = client
        .recv_response_for(1, Duration::from_secs(10))
        .expect("addRoot 应答");
    assert!(resp["result"]["id"].is_number(), "addRoot 应返回条目: {resp}");
    let root_id = resp["result"]["id"].as_i64().unwrap();

    // roots 列表
    client.request(2, "library.roots", None);
    let resp = client
        .recv_response_for(2, Duration::from_secs(5))
        .expect("roots 应答");
    assert_eq!(resp["result"]["roots"].as_array().unwrap().len(), 1);

    // library.scan（ack → 终态）
    client.request(
        3,
        "library.scan",
        Some(serde_json::json!({ "rootId": root_id })),
    );
    let ack = client
        .recv_response_for(3, Duration::from_secs(5))
        .expect("scan ack");
    assert!(ack["result"]["taskId"].is_string());
    let terminal = client
        .recv_terminal_for(3, Duration::from_secs(30))
        .expect("scan 终态");
    assert_eq!(terminal["result"]["status"], "done", "得到 {terminal}");
    assert!(terminal["result"]["stats"]["seen"].as_u64().unwrap_or(0) >= 2);

    // search.items：FTS 命中 txt 正文 token
    client.request(
        4,
        "search.items",
        Some(serde_json::json!({ "query": "curator" })),
    );
    let resp = client
        .recv_response_for(4, Duration::from_secs(5))
        .expect("search 应答");
    assert!(
        resp["result"]["total"].as_u64().unwrap_or(0) >= 1,
        "应命中至少 1 条: {resp}"
    );

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn invalid_params_rejected() {
    let (db, _) = temp_paths("bad");
    let mut client = Client::spawn(&db);

    // 缺 path → -32602
    client.request(10, "library.addRoot", Some(serde_json::json!({})));
    let resp = client
        .recv_response_for(10, Duration::from_secs(10))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 不存在的 rootId → -32602
    client.request(
        11,
        "library.scan",
        Some(serde_json::json!({ "rootId": 99999 })),
    );
    let resp = client
        .recv_response_for(11, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 空 query → -32602
    client.request(
        12,
        "search.items",
        Some(serde_json::json!({ "query": "  " })),
    );
    let resp = client
        .recv_response_for(12, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 未知方法 → -32601
    client.request(13, "nope", None);
    let resp = client
        .recv_response_for(13, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32601);
}
