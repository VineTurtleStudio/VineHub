//! 测试夹具：假 sidecar 插件（协议 v1）。
//!
//! 仅供 vine-host-core 集成测试使用：
//! - `plugin.info`：握手
//! - `echo`：原样返回 params
//! - `progress.demo`：先发 2 条 `task.progress` 通知，再回终态
//! - `block`：永不响应（测超时）
//! - `crash`：立即退出（测崩溃路径）

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};
use vine_plugin_api::{
    method, parse_request_line, to_line, PluginInfo, RpcError, RpcRequest, RpcResponse,
    ScanStartResult, TaskProgressParams,
};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = parse_request_line(&line) else {
            continue;
        };
        // 忽略宿主通知（无 id）
        let Some(id) = req.id.clone() else { continue };

        match req.method.as_str() {
            method::PLUGIN_INFO => {
                let info = PluginInfo {
                    id: "vine.fake".into(),
                    version: "0.0.1".into(),
                    api_version: "1.0".into(),
                    capabilities: vec![
                        "echo".into(),
                        "progress".into(),
                        "scan".into(),
                        "block".into(),
                        "crash".into(),
                    ],
                };
                reply(&mut stdout, RpcResponse::success(id, info).unwrap());
            }
            "echo" => {
                reply(
                    &mut stdout,
                    RpcResponse::success(id, req.params.unwrap_or(Value::Null)).unwrap(),
                );
            }
            "progress.demo" => {
                for done in 1..=2u64 {
                    let note = RpcRequest::notification(method::TASK_PROGRESS)
                        .with_params(TaskProgressParams {
                            task_id: "t-demo".into(),
                            done,
                            total: 2,
                            msg: Some(format!("第 {done} 步")),
                        })
                        .unwrap();
                    reply(&mut stdout, note);
                }
                reply(
                    &mut stdout,
                    RpcResponse::success(id, ScanStartResult { task_id: "t-demo".into() })
                        .unwrap(),
                );
            }
            "scan.demo" => {
                // 协议 v1 双响应：ack 回 taskId → 进度通知 → 同 id 终态
                let task_id = "t-scan-demo";
                reply(
                    &mut stdout,
                    RpcResponse::success(id.clone(), ScanStartResult { task_id: task_id.into() })
                        .unwrap(),
                );
                for done in 1..=3u64 {
                    let note = RpcRequest::notification(method::TASK_PROGRESS)
                        .with_params(TaskProgressParams {
                            task_id: task_id.into(),
                            done,
                            total: 3,
                            msg: None,
                        })
                        .unwrap();
                    reply(&mut stdout, note);
                }
                reply(
                    &mut stdout,
                    RpcResponse::success(
                        id,
                        json!({
                            "taskId": task_id,
                            "ok": 2,
                            "bad": 1,
                            "cancelled": false,
                        }),
                    )
                    .unwrap(),
                );
            }
            "block" => {
                // 永不响应（另起线程，避免卡住主循环），等待宿主超时后被杀
                std::thread::spawn(|| std::thread::sleep(Duration::from_secs(3600)));
            }
            "crash" => {
                std::process::exit(1);
            }
            other => {
                reply(
                    &mut stdout,
                    RpcResponse::error(
                        Some(id),
                        RpcError::new(
                            vine_plugin_api::codes::METHOD_NOT_FOUND,
                            format!("未知方法: {other}"),
                        ),
                    ),
                );
            }
        }
    }
}

fn reply(stdout: &mut impl Write, message: impl serde::Serialize) {
    let line = to_line(&message).expect("夹具消息应可编码为单行");
    writeln!(stdout, "{line}").expect("写 stdout 失败");
    stdout.flush().expect("flush 失败");
}
