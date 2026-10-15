//! 协议级集成测试：spawn 真实 `vine-palma-cursor --rpc` exe。
//! 覆盖：握手 / png→cur 多尺寸转换 / cur→ani / 错误码。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;
use image::ImageEncoder;

struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl Client {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vine-palma-cursor"))
            .arg("--rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vine-palma-cursor --rpc");

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
        self.rx.recv_timeout(timeout).ok().and_then(|l| serde_json::from_str(&l).ok())
    }

    fn recv_terminal_for(&self, id: i64, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(msg) = self.recv(remaining) else { break };
            if msg["id"] == Value::from(id) && msg.get("method").is_none() {
                let r = &msg["result"];
                let is_terminal = r.get("bytes").is_some()
                    || r.get("target").is_some()
                    || r.get("capabilities").is_some()
                    || r.get("pong").is_some()
                    || r.get("ok").is_some()
                    || r.get("format").is_some();
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
        "vine-cursor-rpc-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    (base.join("in.png"), base)
}

fn make_png(path: &std::path::Path, w: u32, h: u32) {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        if (x + y) % 2 == 0 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 128])
        }
    });
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .expect("encode png");
    std::fs::write(path, buf).unwrap();
}

#[test]
fn handshake_reports_id_and_capabilities() {
    let mut client = Client::spawn();
    client.request(0, "plugin.info", None);
    let resp = client
        .recv_terminal_for(0, Duration::from_secs(5))
        .expect("plugin.info 应有响应");
    assert_eq!(resp["result"]["id"], "vine.cursor");
    assert_eq!(resp["result"]["apiVersion"], "1.0");
    let caps: Vec<String> = resp["result"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(caps.contains(&"convert".to_string()));
}

#[test]
fn png_to_multisize_cur_then_to_ani() {
    let (png_path, base) = temp_paths("convert");
    make_png(&png_path, 64, 64);
    let cur_path = base.join("out.cur");
    let ani_path = base.join("out.ani");

    let mut client = Client::spawn();

    // png → 多尺寸 cur
    client.request(
        1,
        "file.convert",
        Some(serde_json::json!({
            "inputPath": png_path.display().to_string(),
            "outputPath": cur_path.display().to_string(),
            "target": "cur",
            "sizes": [16, 32, 64],
        })),
    );
    let terminal = client
        .recv_terminal_for(1, Duration::from_secs(15))
        .expect("cur 转换终态");
    let result = &terminal["result"];
    assert_eq!(result["target"], "cur", "得到 {result}");
    assert_eq!(result["sourceFormat"], "png");
    assert!(result["bytes"].as_u64().unwrap_or(0) > 0);
    let cur_bytes = std::fs::read(&cur_path).expect("cur 应落盘");
    assert_eq!(&cur_bytes[2..4], &[2, 0], "CUR 魔数 type=2");

    // cur → ani（源 cur 多尺寸 → ani 用解码后各帧；多尺寸 cur 解出的帧尺寸不同，
    // write_ani 拒绝混合尺寸 → 应报错或我们只用单尺寸 cur）
    // 先造一个单尺寸 cur 再转 ani
    let cur1_path = base.join("single.cur");
    client.request(
        2,
        "file.convert",
        Some(serde_json::json!({
            "inputPath": png_path.display().to_string(),
            "outputPath": cur1_path.display().to_string(),
            "target": "cur",
            "sizes": [32],
        })),
    );
    let terminal = client
        .recv_terminal_for(2, Duration::from_secs(15))
        .expect("单尺寸 cur 终态");
    assert_eq!(terminal["result"]["target"], "cur");

    client.request(
        3,
        "file.convert",
        Some(serde_json::json!({
            "inputPath": cur1_path.display().to_string(),
            "outputPath": ani_path.display().to_string(),
            "target": "ani",
        })),
    );
    let terminal = client
        .recv_terminal_for(3, Duration::from_secs(15))
        .expect("ani 转换终态");
    let result = &terminal["result"];
    assert_eq!(result["target"], "ani", "得到 {result}");
    let ani_bytes = std::fs::read(&ani_path).expect("ani 应落盘");
    assert_eq!(&ani_bytes[0..4], b"RIFF");
    assert_eq!(&ani_bytes[8..12], b"ACON");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn invalid_params_rejected() {
    let mut client = Client::spawn();

    // 输入不存在 → -32602
    client.request(
        10,
        "file.convert",
        Some(serde_json::json!({
            "inputPath": "Z:\\no\\such\\file.png",
            "outputPath": "Z:\\no\\such\\out.cur",
            "target": "cur",
        })),
    );
    let resp = client
        .recv_terminal_for(10, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // target 非法 → -32602
    client.request(
        11,
        "file.convert",
        Some(serde_json::json!({
            "inputPath": "whatever.png",
            "outputPath": "whatever.cur",
            "target": "webp",
        })),
    );
    let resp = client
        .recv_terminal_for(11, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    // 未知方法 → -32601
    client.request(12, "nope", None);
    let resp = client
        .recv_terminal_for(12, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32601);
}

/// 32x32 RGBA：全零 + 左上角一个不透明像素。
fn make_rgba(size: u32, seed: u8) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    rgba[0] = seed;
    rgba[1] = 128;
    rgba[2] = 200;
    rgba[3] = 255;
    rgba
}

#[test]
fn export_cur_to_path_then_import_back() {
    let (_, base) = temp_paths("exportcur");
    // 嵌套子目录：验证 handler 自动创建父目录
    let cur_path = base.join("nested").join("dir").join("out.cur");
    let mut client = Client::spawn();

    client.request(
        20,
        "file.exportCur",
        Some(serde_json::json!({
            "path": cur_path.display().to_string(),
            "request": {
                "images": [{
                    "width": 32,
                    "height": 32,
                    "hotspot_x": 4,
                    "hotspot_y": 5,
                    "rgba": make_rgba(32, 255),
                }],
            },
        })),
    );
    let resp = client
        .recv_terminal_for(20, Duration::from_secs(10))
        .expect("file.exportCur 应有响应");
    assert!(resp.get("error").is_none(), "得到 {resp}");
    assert_eq!(resp["result"]["path"], cur_path.display().to_string());
    assert!(resp["result"]["bytes"].as_u64().unwrap_or(0) > 0);

    let on_disk = std::fs::read(&cur_path).expect("cur 应落盘（父目录自动创建）");
    assert_eq!(&on_disk[0..4], &[0, 0, 2, 0], "ICO/CUR 魔数 reserved=0 type=2");

    // 同一连接读回
    client.request(
        21,
        "file.importPath",
        Some(serde_json::json!({ "path": cur_path.display().to_string() })),
    );
    let resp = client
        .recv_terminal_for(21, Duration::from_secs(10))
        .expect("file.importPath 应有响应");
    assert!(resp.get("error").is_none(), "得到 {resp}");
    assert_eq!(resp["result"]["format"], "cur");
    let frames = resp["result"]["frames"].as_array().expect("frames 数组");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["width"], 32);
    assert_eq!(frames[0]["height"], 32);
    assert_eq!(frames[0]["hotspot_x"], 4);
    assert_eq!(frames[0]["hotspot_y"], 5);
    let rgba = frames[0]["rgba"].as_array().expect("rgba 数组");
    assert_eq!(rgba.len(), 32 * 32 * 4);
    assert_eq!(rgba[3], 255, "首像素应保持不透明");

    // 路径不存在 → -32602
    client.request(
        22,
        "file.importPath",
        Some(serde_json::json!({ "path": base.join("nope.cur").display().to_string() })),
    );
    let resp = client
        .recv_terminal_for(22, Duration::from_secs(5))
        .expect("应有错误响应");
    assert_eq!(resp["error"]["code"], -32602);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn export_ani_to_path_then_import_back() {
    let (_, base) = temp_paths("exportani");
    let ani_path = base.join("out.ani");
    let mut client = Client::spawn();

    client.request(
        30,
        "file.exportAni",
        Some(serde_json::json!({
            "path": ani_path.display().to_string(),
            "request": {
                "frames": [
                    {
                        "width": 32,
                        "height": 32,
                        "hotspot_x": 1,
                        "hotspot_y": 2,
                        "rgba": make_rgba(32, 255),
                        "delay_ms": 50,
                    },
                    {
                        "width": 32,
                        "height": 32,
                        "hotspot_x": 3,
                        "hotspot_y": 4,
                        "rgba": make_rgba(32, 60),
                        "delay_ms": 100,
                    },
                ],
            },
        })),
    );
    let resp = client
        .recv_terminal_for(30, Duration::from_secs(10))
        .expect("file.exportAni 应有响应");
    assert!(resp.get("error").is_none(), "得到 {resp}");
    assert!(resp["result"]["bytes"].as_u64().unwrap_or(0) > 0);

    let on_disk = std::fs::read(&ani_path).expect("ani 应落盘");
    assert_eq!(&on_disk[0..4], b"RIFF");
    assert_eq!(&on_disk[8..12], b"ACON");

    client.request(
        31,
        "file.importPath",
        Some(serde_json::json!({ "path": ani_path.display().to_string() })),
    );
    let resp = client
        .recv_terminal_for(31, Duration::from_secs(10))
        .expect("file.importPath 应有响应");
    assert!(resp.get("error").is_none(), "得到 {resp}");
    assert_eq!(resp["result"]["format"], "ani");
    let frames = resp["result"]["frames"].as_array().expect("frames 数组");
    assert_eq!(frames.len(), 2);
    for frame in frames {
        assert_eq!(frame["width"], 32);
        assert_eq!(frame["height"], 32);
    }
    // 50ms/100ms 是 60Hz jiffies 的整倍数，往返无损
    assert_eq!(frames[0]["delay_ms"], 50);
    assert_eq!(frames[1]["delay_ms"], 100);
    assert_eq!(frames[0]["hotspot_x"], 1);
    assert_eq!(frames[1]["hotspot_x"], 3);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn import_file_bytes_and_capabilities() {
    let (png_path, base) = temp_paths("importbytes");
    make_png(&png_path, 32, 32);
    let png_bytes = std::fs::read(&png_path).unwrap();

    let mut client = Client::spawn();

    // 握手能力含 export/import
    client.request(40, "plugin.info", None);
    let resp = client
        .recv_terminal_for(40, Duration::from_secs(5))
        .expect("plugin.info 应有响应");
    let caps: Vec<String> = resp["result"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    for expected in ["convert", "export", "import"] {
        assert!(caps.contains(&expected.to_string()), "缺少能力 {expected}: {caps:?}");
    }

    // 内存字节导入（bridge 拖放路径）
    client.request(
        41,
        "import.file",
        Some(serde_json::json!({ "data": png_bytes })),
    );
    let resp = client
        .recv_terminal_for(41, Duration::from_secs(10))
        .expect("import.file 应有响应");
    assert!(resp.get("error").is_none(), "得到 {resp}");
    assert_eq!(resp["result"]["format"], "png");
    let frames = resp["result"]["frames"].as_array().expect("frames 数组");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["width"], 32);

    let _ = std::fs::remove_dir_all(&base);
}
