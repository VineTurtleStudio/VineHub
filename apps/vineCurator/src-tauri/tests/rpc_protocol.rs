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

#[test]
fn state_roundtrip() {
    let (db, _) = temp_paths("state");
    let mut client = Client::spawn(&db);

    // db.info → path 字符串
    client.request(20, "db.info", None);
    let resp = client
        .recv_response_for(20, Duration::from_secs(10))
        .expect("db.info 应答");
    assert!(resp["result"]["path"].is_string());

    // settings.get → 数组（空库无设置也返回 []）
    client.request(21, "settings.get", None);
    let resp = client
        .recv_response_for(21, Duration::from_secs(5))
        .expect("settings.get 应答");
    assert!(resp["result"].is_array(), "settings.get 应返回数组: {resp}");

    // settings.save → {} 后 settings.get 能读回
    client.request(
        22,
        "settings.save",
        Some(serde_json::json!({ "entries": [{ "key": "beets_db_path", "value": "C:/beets/library.db" }] })),
    );
    let resp = client
        .recv_response_for(22, Duration::from_secs(5))
        .expect("settings.save 应答");
    assert!(resp["error"].is_null() || !resp.get("error").map(|e| !e.is_null()).unwrap_or(false));
    client.request(23, "settings.get", None);
    let resp = client
        .recv_response_for(23, Duration::from_secs(5))
        .expect("settings.get 应答");
    let entries = resp["result"].as_array().expect("应为数组");
    assert!(
        entries.iter().any(|e| e["key"] == "beets_db_path" && e["value"] == "C:/beets/library.db"),
        "保存的设置应可读回: {resp}"
    );

    // session.get（未登录）→ null
    client.request(24, "session.get", None);
    let resp = client
        .recv_response_for(24, Duration::from_secs(5))
        .expect("session.get 应答");
    assert!(resp["result"].is_null(), "未登录 session.get 应为 null: {resp}");

    // users.list（空库）→ 数组
    client.request(25, "users.list", None);
    let resp = client
        .recv_response_for(25, Duration::from_secs(5))
        .expect("users.list 应答");
    assert_eq!(resp["result"].as_array().unwrap().len(), 0);

    // dupes.list → 数组（空库无重复组）
    client.request(26, "dupes.list", None);
    let resp = client
        .recv_response_for(26, Duration::from_secs(5))
        .expect("dupes.list 应答");
    assert!(resp["result"].is_array());

    // ocr.pending → {pending: 0}
    client.request(
        27,
        "ocr.pending",
        Some(serde_json::json!({})),
    );
    let resp = client
        .recv_response_for(27, Duration::from_secs(5))
        .expect("ocr.pending 应答");
    assert_eq!(resp["result"]["pending"], 0);

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn category_tag_item_flow() {
    let (db, lib) = temp_paths("cat");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("SongA.mp3"), b"ID3 fake audio content").unwrap();
    std::fs::write(lib.join("curator-notes.txt"), b"plain text note").unwrap();

    let mut client = Client::spawn(&db);

    // addRoot + scan
    client.request(
        1,
        "library.addRoot",
        Some(serde_json::json!({ "path": lib.display().to_string() })),
    );
    let resp = client
        .recv_response_for(1, Duration::from_secs(10))
        .expect("addRoot 应答");
    let root_id = resp["result"]["id"].as_i64().unwrap();

    client.request(
        2,
        "library.scan",
        Some(serde_json::json!({ "rootId": root_id })),
    );
    let _ = client
        .recv_response_for(2, Duration::from_secs(5))
        .expect("scan ack");
    let terminal = client
        .recv_terminal_for(2, Duration::from_secs(30))
        .expect("scan 终态");
    assert_eq!(terminal["result"]["status"], "done");

    // items.list → total >= 2（mp3 + txt）
    client.request(
        3,
        "items.list",
        Some(serde_json::json!({ "rootId": root_id, "limit": 100 })),
    );
    let resp = client
        .recv_response_for(3, Duration::from_secs(5))
        .expect("items.list 应答");
    assert!(
        resp["result"]["total"].as_u64().unwrap_or(0) >= 2,
        "items.list total 应 >= 2: {resp}"
    );
    let first_item_id = resp["result"]["items"][0]["id"].as_i64().unwrap();

    // category.create → {id}
    client.request(
        4,
        "category.create",
        Some(serde_json::json!({ "name": "音乐" })),
    );
    let resp = client
        .recv_response_for(4, Duration::from_secs(5))
        .expect("category.create 应答");
    let cat_id = resp["result"]["id"].as_i64().unwrap();

    // categories.list → 1 个节点
    client.request(5, "categories.list", None);
    let resp = client
        .recv_response_for(5, Duration::from_secs(5))
        .expect("categories.list 应答");
    assert_eq!(resp["result"].as_array().unwrap().len(), 1);

    // tag.create → {id}
    client.request(
        6,
        "tag.create",
        Some(serde_json::json!({ "name": "现场" })),
    );
    let resp = client
        .recv_response_for(6, Duration::from_secs(5))
        .expect("tag.create 应答");
    let _tag_id = resp["result"]["id"].as_i64().unwrap();

    // tags.list → 1 个标签
    client.request(7, "tags.list", None);
    let resp = client
        .recv_response_for(7, Duration::from_secs(5))
        .expect("tags.list 应答");
    assert_eq!(resp["result"].as_array().unwrap().len(), 1);

    // item.categories.set + item.tags.set
    client.request(
        8,
        "item.categories.set",
        Some(serde_json::json!({ "itemId": first_item_id, "categoryIds": [cat_id] })),
    );
    let resp = client
        .recv_response_for(8, Duration::from_secs(5))
        .expect("item.categories.set 应答");
    assert!(resp.get("error").map(|e| e.is_null()).unwrap_or(true), "应无错误: {resp}");

    client.request(
        9,
        "item.tags.set",
        Some(serde_json::json!({ "itemIds": [first_item_id], "tagNames": ["现场"] })),
    );
    let resp = client
        .recv_response_for(9, Duration::from_secs(5))
        .expect("item.tags.set 应答");
    assert!(resp.get("error").map(|e| e.is_null()).unwrap_or(true), "应无错误: {resp}");

    // item.get 详情带分类与标签
    client.request(
        10,
        "item.get",
        Some(serde_json::json!({ "itemId": first_item_id })),
    );
    let resp = client
        .recv_response_for(10, Duration::from_secs(5))
        .expect("item.get 应答");
    let cats = resp["result"]["category_ids"].as_array().expect("category_ids 应为数组");
    assert_eq!(cats.len(), 1, "条目应有 1 个分类: {resp}");
    let tags_arr = resp["result"]["tags"].as_array().expect("tags 应为数组");
    assert_eq!(tags_arr.len(), 1, "条目应有 1 个标签: {resp}");

    // 同名分类重复创建 → -32602
    client.request(
        11,
        "category.create",
        Some(serde_json::json!({ "name": "音乐" })),
    );
    let resp = client
        .recv_response_for(11, Duration::from_secs(5))
        .expect("重复分类应答");
    assert_eq!(resp["error"]["code"], -32602);

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn auth_setup_and_login_flow() {
    let (db, _) = temp_paths("auth");
    let mut client = Client::spawn(&db);

    // admin.setup → user{id, username, role, created_at}，登录即生效
    client.request(
        1,
        "admin.setup",
        Some(serde_json::json!({ "username": "admin", "password": "secret123" })),
    );
    let resp = client
        .recv_response_for(1, Duration::from_secs(10))
        .expect("admin.setup 应答");
    assert!(resp.get("error").map(|e| e.is_null()).unwrap_or(true), "应无错误: {resp}");
    let user_id = resp["result"]["id"].as_i64().unwrap();
    assert_eq!(resp["result"]["username"], "admin");
    assert_eq!(resp["result"]["role"], "admin");

    // session.get → user
    client.request(2, "session.get", None);
    let resp = client
        .recv_response_for(2, Duration::from_secs(5))
        .expect("session.get 应答");
    assert!(resp["result"].is_object(), "setup 后应有会话: {resp}");
    assert_eq!(resp["result"]["id"], user_id);

    // users.list → 1 个
    client.request(3, "users.list", None);
    let resp = client
        .recv_response_for(3, Duration::from_secs(5))
        .expect("users.list 应答");
    assert_eq!(resp["result"].as_array().unwrap().len(), 1);

    // 重复 setup（已有用户）→ 错误
    client.request(
        4,
        "admin.setup",
        Some(serde_json::json!({ "username": "admin2", "password": "secret456" })),
    );
    let resp = client
        .recv_response_for(4, Duration::from_secs(5))
        .expect("重复 setup 应答");
    assert!(resp.get("error").map(|e| !e.is_null()).unwrap_or(false), "重复 setup 应报错: {resp}");

    // auth.logout → session null
    client.request(5, "auth.logout", None);
    let resp = client
        .recv_response_for(5, Duration::from_secs(5))
        .expect("auth.logout 应答");
    assert!(resp.get("error").map(|e| e.is_null()).unwrap_or(true));
    client.request(6, "session.get", None);
    let resp = client
        .recv_response_for(6, Duration::from_secs(5))
        .expect("session.get 应答");
    assert!(resp["result"].is_null(), "登出后应无会话: {resp}");

    // auth.login 正确密码 → user
    client.request(
        7,
        "auth.login",
        Some(serde_json::json!({ "username": "admin", "password": "secret123" })),
    );
    let resp = client
        .recv_response_for(7, Duration::from_secs(10))
        .expect("auth.login 应答");
    assert!(resp.get("error").map(|e| e.is_null()).unwrap_or(true), "应无错误: {resp}");
    assert_eq!(resp["result"]["username"], "admin");

    // auth.logout 再登录错误密码 → -32003
    client.request(8, "auth.logout", None);
    let _ = client
        .recv_response_for(8, Duration::from_secs(5))
        .expect("auth.logout 应答");
    client.request(
        9,
        "auth.login",
        Some(serde_json::json!({ "username": "admin", "password": "wrongpass" })),
    );
    let resp = client
        .recv_response_for(9, Duration::from_secs(10))
        .expect("错误密码应答");
    assert_eq!(resp["error"]["code"], -32003, "错误密码应 PERMISSION_DENIED: {resp}");

    // audit.list → 有 setup/login 记录
    client.request(10, "audit.list", Some(serde_json::json!({ "limit": 50 })));
    let resp = client
        .recv_response_for(10, Duration::from_secs(5))
        .expect("audit.list 应答");
    let entries = resp["result"].as_array().expect("audit.list 应为数组");
    assert!(!entries.is_empty(), "应有审计记录");
    assert!(
        entries.iter().any(|e| e["action"] == "setup"),
        "应有 setup 审计: {resp}"
    );
    assert!(
        entries.iter().any(|e| e["action"] == "login"),
        "应有 login 审计: {resp}"
    );

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}
