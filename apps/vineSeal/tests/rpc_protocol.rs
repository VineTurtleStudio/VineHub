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
        let mut child = Command::new(env!("CARGO_BIN_EXE_vine-seal"))
            .arg("--rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vine-seal --rpc");

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

    fn request(&mut self, id: i64, method: &str, params: Option<Value>) {
        let mut req = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            req["params"] = p;
        }
        let line = serde_json::to_string(&req).unwrap();
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&self, timeout: Duration) -> Option<Value> {
        self.rx
            .recv_timeout(timeout)
            .ok()
            .and_then(|l| serde_json::from_str(&l).ok())
    }

    fn recv_response_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == id && msg.get("method").is_none() {
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
        "vine-seal-rpc-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn handshake_returns_plugin_info() {
    let mut client = Client::spawn();
    client.request(0, "plugin.info", None);
    let resp = client
        .recv_response_for(0, Duration::from_secs(5))
        .expect("应收到 plugin.info 响应");

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["result"]["id"], "vine.seal");
    assert_eq!(resp["result"]["apiVersion"], "1.0");
    let caps = resp["result"]["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "transfer"));
    assert!(caps.iter().any(|c| c == "verify"));
}

#[test]
fn ping_returns_pong() {
    let mut client = Client::spawn();
    client.request(1, "ping", None);
    let resp = client
        .recv_response_for(1, Duration::from_secs(5))
        .expect("应收到 ping 响应");
    assert_eq!(resp["result"]["pong"], true);
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
fn transfer_start_progress_and_terminal() {
    let dir = temp_dir("transfer");
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("a.txt"), b"aaaaa").unwrap();
    std::fs::write(src_dir.join("b.txt"), b"bbbbb").unwrap();
    let dst = dir.join("dst");
    std::fs::create_dir_all(&dst).unwrap();

    let mut client = Client::spawn();
    client.request(
        1,
        "transfer.start",
        Some(serde_json::json!({
            "sources": [src_dir.to_string_lossy()],
            "dest": dst.to_string_lossy(),
            "mode": "copy",
        })),
    );

    let ack = client
        .recv_response_for(1, Duration::from_secs(5))
        .expect("应收到 ack");
    let task_id = ack["result"]["taskId"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("t-"));

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut progress_seen = 0u32;
    let terminal = loop {
        assert!(std::time::Instant::now() < deadline, "等待终态超时");
        let msg = client.recv(Duration::from_secs(10)).expect("连接不应断开");
        if msg.get("method").is_some() {
            assert_eq!(msg["method"], "task.progress");
            assert_eq!(msg["params"]["taskId"], task_id.as_str());
            progress_seen += 1;
            continue;
        }
        if msg["id"] == 1 {
            break msg;
        }
    };

    assert!(progress_seen >= 1, "应至少收到一条进度通知");
    let result = &terminal["result"];
    assert_eq!(result["taskId"], task_id.as_str());
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["bad"], 0);
    assert!(result["ok"].as_u64().unwrap() >= 3);

    assert!(dst.join("src/a.txt").exists());
    assert!(dst.join("src/b.txt").exists());
    assert_eq!(std::fs::read(dst.join("src/a.txt")).unwrap(), b"aaaaa");

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
fn verify_start_and_terminal() {
    let dir = temp_dir("verify");
    std::fs::write(dir.join("a.txt"), b"hello").unwrap();
    let hash = {
        let child = Command::new(env!("CARGO_BIN_EXE_vine-seal"))
            .args(["hash", dir.join("a.txt").to_str().unwrap()])
            .output()
            .expect("run vine-seal hash");
        let text = String::from_utf8_lossy(&child.stdout);
        child.status.success().then(|| text.split_whitespace().next().unwrap().to_string())
    }
    .expect("hash 子命令应成功");
    let manifest = dir.join("list.sha256");
    std::fs::write(&manifest, format!("{hash} *a.txt\n")).unwrap();

    let mut client = Client::spawn();
    client.request(
        1,
        "verify.start",
        Some(serde_json::json!({
            "manifest": manifest.to_string_lossy(),
            "base": dir.to_string_lossy(),
            "algo": "sha256",
        })),
    );
    let ack = client
        .recv_response_for(1, Duration::from_secs(5))
        .expect("应收到 ack");
    let task_id = ack["result"]["taskId"].as_str().unwrap().to_string();

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let terminal = loop {
        assert!(std::time::Instant::now() < deadline, "等待终态超时");
        let msg = client.recv(Duration::from_secs(10)).expect("连接不应断开");
        if msg.get("method").is_none() && msg["id"] == 1 {
            break msg;
        }
    };
    let result = &terminal["result"];
    assert_eq!(result["taskId"], task_id.as_str());
    assert_eq!(result["ok"], 1);
    assert_eq!(result["mismatch"], 0);
    assert_eq!(result["missing"], 0);
}

#[test]
fn rejects_invalid_params() {
    let mut client = Client::spawn();

    client.request(1, "transfer.start", Some(serde_json::json!({ "sources": [] })));
    let resp = client.recv_response_for(1, Duration::from_secs(5)).unwrap();
    assert_eq!(resp["error"]["code"], -32602);

    client.request(2, "transfer.start", Some(serde_json::json!({})));
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
