//! 大批量 / 深目录同步完整性回归：目标端必须与源端逐文件一致。
//! 覆盖：文件数量大（多目录、多文件）、路径深度超过 MAX_PATH(260)。

use std::collections::BTreeSet;
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
        "vs-scale-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 递归列出（相对路径, 字节数）；用 verbatim 根绕开 260 限制
fn list_files(root: &Path) -> BTreeSet<(String, u64)> {
    let long = PathBuf::from(format!(r"\\?\{}", root.display()));
    let mut out = BTreeSet::new();
    let mut stack = vec![long];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() {
                stack.push(path);
            } else if md.is_file() {
                let rel = path
                    .strip_prefix(&PathBuf::from(format!(r"\\?\{}", root.display())))
                    .map(|p| p.to_string_lossy().replace('/', "\\"))
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                out.insert((rel, md.len()));
            }
        }
    }
    out
}

#[test]
fn extreme_depth_paths_are_all_copied() {
    // 150 层 × 20 字符组件 ≈ 3000+ 字符，远超 MAX_PATH，仍应逐文件同步
    let src = temp_dir("deep-src");
    let dst = temp_dir("deep-dst");
    let mut cur = src.to_path_buf();
    let mut rel = String::new();
    let mut expected: BTreeSet<(String, u64)> = BTreeSet::new();
    for i in 0..150 {
        let seg = format!("深度层级{i:03}abcdefghij");
        cur.push(&seg);
        rel.push_str(&seg);
        rel.push('\\');
        if i % 10 == 9 || i == 149 {
            std::fs::create_dir_all(PathBuf::from(format!(r"\\?\{}", cur.display()))).unwrap();
            let body = format!("payload-{i}");
            let name = format!("f{i:03}.txt");
            std::fs::write(
                PathBuf::from(format!(r"\\?\{}", cur.join(&name).display())),
                &body,
            )
            .unwrap();
            expected.insert((format!("{rel}{name}"), body.len() as u64));
        }
    }
    let deepest = cur.display().to_string().len();
    assert!(deepest > 2500, "最深层路径应远超 MAX_PATH，实际 {deepest}");

    let mut client = Client::spawn();
    client.request(
        1,
        "config.save",
        Some(serde_json::json!({
            "config": { "copyThreads": 4, "pairs": [{
                "id": "p1", "name": "deep",
                "source": src.display().to_string(),
                "dest": dst.display().to_string(),
                "enabled": true, "include": [], "exclude": [], "intervalMinutes": null
            }] }
        })),
    );
    let resp = client.recv_response_for(1, Duration::from_secs(10)).expect("config.save");
    assert!(resp.get("error").is_none(), "config.save 失败: {resp}");

    client.request(2, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(300)).expect("sync 终态");
    let result = &terminal["result"];
    assert_eq!(result["ok"], true, "同步应成功: {result}");
    assert_eq!(
        result["counts"]["added"].as_u64().unwrap() as usize,
        expected.len(),
        "新增数量应等于源文件数: {result}"
    );

    let actual = list_files(&dst);
    let missing: Vec<_> = expected.difference(&actual).cloned().collect();
    assert!(
        missing.is_empty(),
        "深层路径遗漏 {} 个文件，例如: {:?}",
        missing.len(),
        missing.iter().take(3).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(PathBuf::from(format!(r"\\?\{}", src.display())));
    let _ = std::fs::remove_dir_all(PathBuf::from(format!(r"\\?\{}", dst.display())));
}

#[test]
fn many_files_in_one_directory_are_all_copied() {
    let src = temp_dir("flat-src");
    let dst = temp_dir("flat-dst");
    let dir = src.join("大量文件");
    std::fs::create_dir_all(&dir).unwrap();

    const N: usize = 20_000;
    for i in 0..N {
        std::fs::write(dir.join(format!("f{i:05}.dat")), i.to_string()).unwrap();
    }

    let mut client = Client::spawn();
    client.request(
        1,
        "config.save",
        Some(serde_json::json!({
            "config": { "copyThreads": 8, "pairs": [{
                "id": "p1", "name": "flat",
                "source": src.display().to_string(),
                "dest": dst.display().to_string(),
                "enabled": true, "include": [], "exclude": [], "intervalMinutes": null
            }] }
        })),
    );
    let resp = client.recv_response_for(1, Duration::from_secs(10)).expect("config.save");
    assert!(resp.get("error").is_none(), "config.save 失败: {resp}");

    client.request(2, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(600)).expect("sync 终态");
    let result = &terminal["result"];
    assert_eq!(result["ok"], true, "同步应成功: {result}");
    assert_eq!(result["counts"]["failed"], 0, "不应有失败项: {result}");
    assert_eq!(
        result["counts"]["added"].as_u64().unwrap() as usize,
        N,
        "新增数量应等于源文件数: {result}"
    );

    let actual = list_files(&dst);
    assert_eq!(actual.len(), N, "目标端文件数应等于源端文件数");
    let missing: Vec<_> = (0..N)
        .map(|i| format!("大量文件\\f{i:05}.dat"))
        .filter(|rel| !actual.iter().any(|(r, _)| r == rel))
        .take(5)
        .collect();
    assert!(missing.is_empty(), "目标端遗漏文件，例如: {missing:?}");

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

#[test]
fn many_files_and_deep_paths_are_all_copied() {
    let src = temp_dir("src");
    let dst = temp_dir("dst");

    // 1) 文件多：200 个目录 × 30 个文件 = 6000
    let mut expected: BTreeSet<(String, u64)> = BTreeSet::new();
    for d in 0..200 {
        let dir = src.join(format!("d{d:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..30 {
            let name = format!("f{f:02}.bin");
            let body = format!("{d}-{f}");
            std::fs::write(dir.join(&name), &body).unwrap();
            expected.insert((format!("d{d:03}\\{name}"), body.len() as u64));
        }
    }

    // 2) 深度：25 层 × 9 字符组件，超出 MAX_PATH(260)
    let mut deep = src.to_path_buf();
    let mut rel_deep = String::new();
    for i in 0..25 {
        let seg = format!("level{i:02}ab");
        deep.push(&seg);
        rel_deep.push_str(&seg);
        rel_deep.push('\\');
    }
    std::fs::create_dir_all(PathBuf::from(format!(r"\\?\{}", deep.display()))).unwrap();
    let deep_len = deep.display().to_string().len() + 12;
    assert!(deep_len > 260, "深层路径应超过 MAX_PATH，实际 {deep_len}");
    for f in 0..3 {
        let body = format!("deep-{f}");
        let path = PathBuf::from(format!(r"\\?\{}", deep.join(format!("deep{f}.txt")).display()));
        std::fs::write(&path, &body).unwrap();
        expected.insert((format!("{rel_deep}deep{f}.txt"), body.len() as u64));
    }

    assert_eq!(list_files(&src).len(), expected.len(), "源端自身枚举应与预期一致");

    let mut client = Client::spawn();
    client.request(
        1,
        "config.save",
        Some(serde_json::json!({
            "config": { "copyThreads": 8, "pairs": [{
                "id": "p1", "name": "scale",
                "source": src.display().to_string(),
                "dest": dst.display().to_string(),
                "enabled": true, "include": [], "exclude": [], "intervalMinutes": null
            }] }
        })),
    );
    let resp = client.recv_response_for(1, Duration::from_secs(10)).expect("config.save");
    assert!(resp.get("error").is_none(), "config.save 失败: {resp}");

    client.request(2, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(300)).expect("sync 终态");
    let result = &terminal["result"];
    assert_eq!(result["ok"], true, "同步应成功: {result}");
    assert_eq!(result["counts"]["failed"], 0, "不应有失败项: {result}");
    assert_eq!(
        result["counts"]["added"].as_u64().unwrap() as usize,
        expected.len(),
        "新增数量应等于源文件数: {result}"
    );

    // 目标端逐文件核对（含深层路径）
    let actual = list_files(&dst);
    let missing: Vec<_> = expected.difference(&actual).cloned().collect();
    let extra: Vec<_> = actual.difference(&expected).cloned().collect();
    assert!(
        missing.is_empty(),
        "目标端遗漏 {} 个文件，例如: {:?}",
        missing.len(),
        missing.iter().take(5).collect::<Vec<_>>()
    );
    assert!(
        extra.is_empty(),
        "目标端多出 {} 个文件，例如: {:?}",
        extra.len(),
        extra.iter().take(5).collect::<Vec<_>>()
    );

    // 再同步一次：不应重复复制（mtime 已对齐）
    client.request(3, "sync.preview", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(3, Duration::from_secs(300)).expect("preview 终态");
    assert_eq!(
        terminal["result"]["changeCount"], 0,
        "二次同步不应有变更: {terminal}"
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

/// 目录联接/符号链接会被跳过（设计如此），但必须在结果里明确说明，
/// 否则用户只会看到「文件没备份过去」而不知道原因。
#[test]
fn junctions_are_skipped_and_reported() {
    let src = temp_dir("junc-src");
    let dst = temp_dir("junc-dst");
    std::fs::create_dir_all(src.join("real")).unwrap();
    std::fs::write(src.join("real").join("a.txt"), "AAA").unwrap();
    std::fs::write(src.join("top.txt"), "TOP").unwrap();

    // 建一个目录联接：指向 real
    let link = src.join("link");
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(src.join("real"))
        .status()
        .expect("mklink");
    assert!(status.success(), "创建目录联接失败");

    let mut client = Client::spawn();
    client.request(
        1,
        "config.save",
        Some(serde_json::json!({
            "config": { "copyThreads": 2, "pairs": [{
                "id": "p1", "name": "junc",
                "source": src.display().to_string(),
                "dest": dst.display().to_string(),
                "enabled": true, "include": [], "exclude": [], "intervalMinutes": null
            }] }
        })),
    );
    let resp = client.recv_response_for(1, Duration::from_secs(10)).expect("config.save");
    assert!(resp.get("error").is_none(), "config.save 失败: {resp}");

    // 预览：必须说明「跳过了符号链接/目录联接」
    client.request(2, "sync.preview", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(2, Duration::from_secs(60)).expect("preview 终态");
    let notes = terminal["result"]["scanNotes"].as_array().cloned().unwrap_or_default();
    let joined = notes.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" | ");
    assert!(
        joined.contains("符号链接") || joined.contains("symlink") || joined.contains("联接") || joined.contains("junction"),
        "预览应说明跳过了目录联接，实际 scanNotes={joined:?}"
    );
    // 联接内的文件不参与同步；真实文件正常计入
    assert_eq!(terminal["result"]["changeCount"], 2, "只应统计 real 下的 a.txt 与 top.txt");

    client.request(3, "sync.start", Some(serde_json::json!({ "pairId": "p1" })));
    let terminal = client.recv_terminal_for(3, Duration::from_secs(120)).expect("sync 终态");
    assert_eq!(terminal["result"]["ok"], true);
    assert_eq!(terminal["result"]["counts"]["added"], 2);
    assert!(dst.join("real").join("a.txt").is_file());
    assert!(!dst.join("link").exists(), "目录联接不应被复制");

    let _ = Command::new("cmd").args(["/C", "rmdir"]).arg(&link).status();
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}