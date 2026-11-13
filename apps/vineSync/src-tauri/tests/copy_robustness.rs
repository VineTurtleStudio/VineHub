//! 目标端「拒绝访问 (os error 5)」类问题的端到端回归：
//! 残留 tmp 对象、只读源文件、目录被文件占位。

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

    /// 任意响应（config.save 这类同步方法用）
    fn recv_response_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(line) = self.rx.recv_timeout(remaining) else { break };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
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
            let Ok(line) = self.rx.recv_timeout(remaining) else { break };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg["id"] == id && msg.get("method").is_none() {
                let r = &msg["result"];
                if r.get("counts").is_some() || r.get("changeCount").is_some() {
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
        "vs-copy-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save_pair(client: &mut Client, src: &PathBuf, dst: &PathBuf) {
    client.request(
        1,
        "config.save",
        Some(serde_json::json!({
            "config": { "copyThreads": 2, "pairs": [{
                "id": "p1", "name": "t",
                "source": src.display().to_string(),
                "dest": dst.display().to_string(),
                "enabled": true, "include": [], "exclude": [], "intervalMinutes": null
            }] }
        })),
    );
    let resp = client.recv_response_for(1, Duration::from_secs(10)).expect("config.save 应有响应");
    assert!(resp.get("error").is_none(), "config.save 失败: {resp}");
}

/// 只读源文件：一轮同步后目标 mtime 与源一致，第二轮预览 0 变更（不再反复复制）
#[test]
fn readonly_source_converges_after_first_sync() {
    let src = temp_dir("ro-src");
    let dst = temp_dir("ro-dst");
    std::fs::write(src.join("归档.csv"), "A,B\n1,2\n").unwrap();
    let mut perms = std::fs::metadata(src.join("归档.csv")).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(src.join("归档.csv"), perms).unwrap();

    let mut client = Client::spawn();
    save_pair(&mut client, &src, &dst);

    client.request(2, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(60)).expect("sync 终态");
    assert_eq!(terminal["result"]["ok"], true, "只读源文件应能复制: {terminal}");
    assert_eq!(terminal["result"]["counts"]["added"], 1);

    let dst_file = dst.join("归档.csv");
    assert_eq!(std::fs::read_to_string(&dst_file).unwrap(), "A,B\n1,2\n");
    assert!(
        std::fs::metadata(&dst_file).unwrap().permissions().readonly(),
        "目标应保留源文件只读语义"
    );

    // 关键断言：mtime 已对齐 → 没有重复复制
    client.request(3, "sync.preview", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(3, Duration::from_secs(30)).expect("preview 终态");
    assert_eq!(
        terminal["result"]["changeCount"], 0,
        "只读源文件不应每轮都被判定为更新: {terminal}"
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

/// 目标端残留 `<文件>.vinesync.tmp` 目录：不阻塞复制，也不残留在结果里
#[test]
fn stale_tmp_directory_does_not_block_sync() {
    let src = temp_dir("tmp-src");
    let dst = temp_dir("tmp-dst");
    std::fs::create_dir_all(src.join("_管理")).unwrap();
    std::fs::write(src.join("_管理").join("作品总览.csv"), "作品,总览\n").unwrap();
    // 上次中断留下的临时目录
    let stale = dst.join("_管理").join("作品总览.csv.vinesync.tmp");
    std::fs::create_dir_all(stale.join("nested")).unwrap();
    std::fs::write(stale.join("nested").join("junk.bin"), "junk").unwrap();

    let mut client = Client::spawn();
    save_pair(&mut client, &src, &dst);
    client.request(2, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(60)).expect("sync 终态");
    let result = &terminal["result"];
    assert_eq!(result["ok"], true, "残留 tmp 目录不应导致失败: {result}");
    assert_eq!(result["counts"]["failed"], 0, "{result}");
    assert_eq!(
        std::fs::read_to_string(dst.join("_管理").join("作品总览.csv")).unwrap(),
        "作品,总览\n"
    );
    assert!(!stale.exists(), "残留 tmp 目录应被清理");

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}
