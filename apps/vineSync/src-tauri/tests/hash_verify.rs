//! 哈希校验（SHA-256）行为测试：spawn 真实 `vinesync --rpc` exe。
//! 覆盖：同尺寸同 mtime 但内容不同必须被识别；大目录哈希耗时与进度可观测。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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

    fn request(&mut self, id: i64, method: &str, params: Option<Value>) {
        let mut req = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            req["params"] = p;
        }
        writeln!(self.stdin, "{req}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&self, timeout: Duration) -> Option<Value> {
        self.rx.recv_timeout(timeout).ok().and_then(|l| serde_json::from_str(&l).ok())
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

    fn recv_terminal_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == id && msg.get("method").is_none() {
                let r = &msg["result"];
                if r.get("counts").is_some() || r.get("error").is_some() {
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
        "vinesync-hash-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save_pair(client: &mut Client, id: i64, src: &Path, dst: &Path, hash: bool) {
    client.request(
        id,
        "config.save",
        Some(serde_json::json!({
            "config": {
                "hashVerify": hash,
                "copyThreads": 4,
                "pairs": [{
                    "id": "p1",
                    "name": "hash",
                    "source": src.display().to_string(),
                    "dest": dst.display().to_string(),
                    "enabled": true,
                    "include": [],
                    "exclude": [],
                    "intervalMinutes": null
                }]
            }
        })),
    );
    let resp = client.recv_response_for(id, Duration::from_secs(10)).expect("config.save 响应");
    assert!(resp.get("error").is_none(), "config.save 失败: {resp}");
}

/// 同尺寸、同 mtime、内容不同 → 开启哈希必须判为 update
#[test]
fn hash_detects_same_size_same_mtime_corruption() {
    let src = temp_dir("src");
    let dst = temp_dir("dst");
    std::fs::write(src.join("a.bin"), "AAAA").unwrap();
    std::fs::write(dst.join("a.bin"), "BBBB").unwrap();
    // 让两侧尺寸与 mtime 完全一致：只有内容不同
    let t = std::fs::metadata(src.join("a.bin")).unwrap().modified().unwrap();
    let f = std::fs::File::options().write(true).open(dst.join("a.bin")).unwrap();
    f.set_modified(t).unwrap();
    drop(f);

    let mut client = Client::spawn();

    // 关闭哈希：尺寸/mtime 相同 → 0 变更（预期行为）
    save_pair(&mut client, 1, &src, &dst, false);
    client.request(2, "sync.preview", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(30)).expect("preview 终态");
    assert_eq!(terminal["result"]["changeCount"], 0, "关闭哈希时应为 0: {terminal}");

    // 开启哈希：必须发现内容差异
    save_pair(&mut client, 3, &src, &dst, true);
    client.request(4, "sync.preview", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(4, Duration::from_secs(60)).expect("哈希 preview 终态");
    assert_eq!(
        terminal["result"]["counts"]["updated"], 1,
        "开启哈希应判为 1 项更新: {terminal}"
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

/// 大目录开启哈希：耗时应在合理范围，且不应因逐文件串行读两端而爆炸
#[test]
fn hash_preview_on_many_files_stays_fast() {
    let src = temp_dir("many-src");
    let dst = temp_dir("many-dst");
    let payload = vec![7u8; 8192];
    for i in 0..1500 {
        std::fs::write(src.join(format!("f{i:05}.bin")), &payload).unwrap();
    }
    let mut client = Client::spawn();
    save_pair(&mut client, 1, &src, &dst, false);
    client.request(2, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(120)).expect("sync 终态");
    assert_eq!(terminal["result"]["ok"], true, "首轮同步应成功: {terminal}");

    // 全部对齐后开启哈希 preview：只读两端，不应产生变更
    save_pair(&mut client, 3, &src, &dst, true);
    let start = std::time::Instant::now();
    client.request(4, "sync.preview", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(4, Duration::from_secs(120)).expect("哈希 preview 终态");
    let elapsed = start.elapsed();
    assert_eq!(terminal["result"]["changeCount"], 0, "对齐后应为 0 变更: {terminal}");
    // 1500 文件 × 8KB × 2 端 = 24MB，串行 SHA-256 也应 < 5s；超出说明有额外重复读
    assert!(elapsed < Duration::from_secs(5), "哈希 preview 过慢: {elapsed:?}");

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}
