//! Sidecar 子进程管理：spawn、行分隔编解码、id 配对、pending 表、超时、Job 收尾。
//!
//! 约定（规范 v1）：
//! - stdin/stdout 一行一条 UTF-8 JSON；stderr 一律视为日志
//! - 响应必须回带请求 id（配对 = 终态）；通知不带 id
//! - Windows 下 `CREATE_NO_WINDOW` 启动；结束用 `taskkill /T` 防孤儿进程

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use vine_plugin_api::{
    method, parse_request_line, parse_response_line, to_line, ApiVersion, PluginInfo, RequestId,
    RpcError, RpcRequest, HOST_API_VERSION,
};

/// CREATE_NO_WINDOW：避免 sidecar 弹出控制台窗口。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Sidecar 管理 / 通信错误。
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("启动子进程失败: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("握手超时（{0:?}）")]
    HandshakeTimeout(Duration),

    #[error("握手失败: {0}")]
    Handshake(String),

    #[error("插件版本不兼容: {0}")]
    Incompatible(String),

    #[error("请求超时: {method}")]
    Timeout { method: String },

    #[error("子进程已退出")]
    NotRunning,

    #[error("RPC 错误 [{code}] {message}")]
    Rpc { code: i32, message: String },

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("消息编码失败: {0}")]
    Encode(String),
}

impl From<RpcError> for SidecarError {
    fn from(e: RpcError) -> Self {
        Self::Rpc {
            code: e.code,
            message: e.message,
        }
    }
}

/// 拉起 sidecar 的配置。
#[derive(Debug, Clone)]
pub struct SidecarConfig {
    /// 期望握手返回的插件 id（即 manifest id）。
    pub plugin_id: String,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub work_dir: Option<PathBuf>,
    /// 期望插件实现的 apiVersion（取自 manifest）。
    pub expected_api_version: String,
    /// 单次请求超时。
    pub request_timeout: Duration,
    /// 握手（plugin.info）超时。
    pub handshake_timeout: Duration,
}

impl SidecarConfig {
    /// 默认以 `--rpc` 参数启动，请求超时 10s，握手 5s。
    pub fn new(plugin_id: impl Into<String>, exe: impl Into<PathBuf>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            exe: exe.into(),
            args: vec!["--rpc".to_string()],
            work_dir: None,
            expected_api_version: HOST_API_VERSION.to_string(),
            request_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(5),
        }
    }
}

type PendingMap = Arc<Mutex<HashMap<i64, Sender<Result<Value, RpcError>>>>>;

/// 一条已握手的 sidecar 连接。
#[derive(Debug)]
pub struct SidecarHandle {
    plugin_id: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: PendingMap,
    next_id: AtomicI64,
    /// 插件发来的通知/请求（如 `task.progress`）。
    notifications: Receiver<RpcRequest>,
    stderr_log: Arc<Mutex<Vec<String>>>,
    request_timeout: Duration,
    /// 握手返回的信息。
    pub info: PluginInfo,
}

impl SidecarHandle {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// 子进程是否仍在运行。
    pub fn is_running(&self) -> bool {
        match self.child.lock().unwrap().try_wait() {
            Ok(status) => status.is_none(),
            Err(_) => false,
        }
    }

    /// 取走 stderr 日志行（协议外，宿主落盘用）。
    pub fn take_stderr_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.stderr_log.lock().unwrap())
    }

    /// 取一条插件通知（非阻塞）。
    pub fn try_recv_notification(&self) -> Option<RpcRequest> {
        self.notifications.try_recv().ok()
    }

    /// 发送请求并按 id 配对等待终态响应；超时返回 [`SidecarError::Timeout`]。
    pub fn request(&self, method_name: &str, params: Option<Value>) -> Result<Value, SidecarError> {
        if !self.is_running() {
            return Err(SidecarError::NotRunning);
        }
        send_request(
            &self.stdin,
            &self.pending,
            &self.next_id,
            method_name,
            params,
            self.request_timeout,
        )
    }

    /// 发送通知（无 id，不等待响应）。
    pub fn notify(&self, method_name: &str, params: Option<Value>) -> Result<(), SidecarError> {
        let mut req = RpcRequest::notification(method_name);
        req.params = params;
        let line = to_line(&req).map_err(|e| SidecarError::Encode(e.message))?;
        write_line(&self.stdin, &line)
    }

    /// 强杀进程树（taskkill /T，失败回退直接 kill）并回收。
    pub fn kill(&self) {
        let mut child = self.child.lock().unwrap();
        kill_tree(&mut child);
        // 回收所有挂起请求，避免调用方干等超时
        let mut pending = self.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(RpcError::new(
                vine_plugin_api::codes::PLUGIN_INTERNAL,
                "子进程已被终止",
            )));
        }
    }
}

impl Drop for SidecarHandle {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 统一管理多插件 sidecar（P0 骨架：每插件一条连接）。
#[derive(Debug, Default)]
pub struct SidecarManager {
    handles: HashMap<String, Arc<SidecarHandle>>,
}

impl SidecarManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 拉起子进程并完成握手校验；不兼容则杀进程并返回错误。
    pub fn spawn(&mut self, config: SidecarConfig) -> Result<Arc<SidecarHandle>, SidecarError> {
        if self.handles.contains_key(&config.plugin_id) {
            self.kill(&config.plugin_id);
        }

        let mut command = Command::new(&config.exe);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &config.work_dir {
            command.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn().map_err(SidecarError::Spawn)?;
        let stdin = child.stdin.take().expect("stdin 已 piped");
        let stdout = child.stdout.take().expect("stdout 已 piped");
        let stderr = child.stderr.take().expect("stderr 已 piped");

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let stderr_log = Arc::new(Mutex::new(Vec::new()));
        let (notif_tx, notif_rx) = channel::<RpcRequest>();

        spawn_reader_thread(stdout, pending.clone(), notif_tx);
        spawn_stderr_thread(stderr, stderr_log.clone());

        let stdin = Mutex::new(stdin);
        let child = Mutex::new(child);
        let next_id = AtomicI64::new(1);

        // 握手：plugin.info（id 配对）
        let handshake = send_request(
            &stdin,
            &pending,
            &next_id,
            method::PLUGIN_INFO,
            None,
            config.handshake_timeout,
        );
        let info_value = match handshake {
            Ok(value) => value,
            Err(SidecarError::Timeout { .. }) => {
                kill_tree(&mut child.lock().unwrap());
                return Err(SidecarError::HandshakeTimeout(config.handshake_timeout));
            }
            Err(e) => {
                kill_tree(&mut child.lock().unwrap());
                return Err(SidecarError::Handshake(e.to_string()));
            }
        };
        let info: PluginInfo = match serde_json::from_value(info_value) {
            Ok(info) => info,
            Err(e) => {
                kill_tree(&mut child.lock().unwrap());
                return Err(SidecarError::Handshake(format!("plugin.info 结果非法: {e}")));
            }
        };

        if info.id != config.plugin_id {
            kill_tree(&mut child.lock().unwrap());
            return Err(SidecarError::Handshake(format!(
                "插件 id 不匹配: 期望 {}，握手返回 {}",
                config.plugin_id, info.id
            )));
        }
        let actual = match info.api_version.parse::<ApiVersion>() {
            Ok(v) => v,
            Err(_) => {
                kill_tree(&mut child.lock().unwrap());
                return Err(SidecarError::Handshake(format!(
                    "apiVersion 非法: {}",
                    info.api_version
                )));
            }
        };
        let expected = config
            .expected_api_version
            .parse::<ApiVersion>()
            .expect("expected_api_version 是合法版本");
        if !actual.is_compatible_with_host(expected) {
            kill_tree(&mut child.lock().unwrap());
            return Err(SidecarError::Incompatible(format!(
                "插件 apiVersion {} vs 宿主 {}",
                info.api_version, config.expected_api_version
            )));
        }

        let handle = Arc::new(SidecarHandle {
            plugin_id: config.plugin_id.clone(),
            child,
            stdin,
            pending,
            next_id,
            notifications: notif_rx,
            stderr_log,
            request_timeout: config.request_timeout,
            info,
        });
        self.handles.insert(config.plugin_id.clone(), handle.clone());
        Ok(handle)
    }

    pub fn get(&self, plugin_id: &str) -> Option<Arc<SidecarHandle>> {
        self.handles.get(plugin_id).cloned()
    }

    pub fn is_running(&self, plugin_id: &str) -> bool {
        self.handles
            .get(plugin_id)
            .map(|h| h.is_running())
            .unwrap_or(false)
    }

    pub fn kill(&mut self, plugin_id: &str) -> bool {
        if let Some(handle) = self.handles.remove(plugin_id) {
            handle.kill();
            true
        } else {
            false
        }
    }

    pub fn kill_all(&mut self) {
        for (_, handle) in self.handles.drain() {
            handle.kill();
        }
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

/// 杀整棵进程树并回收（Windows: taskkill /T /F）。
pub fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn send_request(
    stdin: &Mutex<ChildStdin>,
    pending: &PendingMap,
    next_id: &AtomicI64,
    method_name: &str,
    params: Option<Value>,
    timeout: Duration,
) -> Result<Value, SidecarError> {
    let id = next_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = channel();
    pending.lock().unwrap().insert(id, tx);

    let mut req = RpcRequest::new(RequestId::Number(id), method_name);
    req.params = params;
    let line = to_line(&req).map_err(|e| SidecarError::Encode(e.message))?;
    if let Err(e) = write_line(stdin, &line) {
        pending.lock().unwrap().remove(&id);
        return Err(e);
    }

    match rx.recv_timeout(timeout) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(rpc_err)) => Err(rpc_err.into()),
        Err(_) => {
            pending.lock().unwrap().remove(&id);
            Err(SidecarError::Timeout {
                method: method_name.to_string(),
            })
        }
    }
}

fn write_line(stdin: &Mutex<ChildStdin>, line: &str) -> Result<(), SidecarError> {
    let mut stdin = stdin.lock().unwrap();
    stdin.write_all(line.as_bytes())?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn spawn_reader_thread(
    stdout: std::process::ChildStdout,
    pending: PendingMap,
    notif_tx: Sender<RpcRequest>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match parse_response_line(&line) {
                Ok(resp) => {
                    if let Some(RequestId::Number(n)) = resp.id {
                        if let Some(tx) = pending.lock().unwrap().remove(&n) {
                            let delivery = match (resp.result, resp.error) {
                                (_, Some(err)) => Err(err),
                                (result, None) => Ok(result.unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(delivery);
                        }
                    }
                }
                Err(_) => {
                    // 非响应行 → 视为插件发来的通知/请求（如 task.progress）
                    if let Ok(req) = parse_request_line(&line) {
                        let _ = notif_tx.send(req);
                    }
                    // 无法归类的行静默丢弃（stderr 才是日志通道）
                }
            }
        }
        // stdout EOF：所有挂起请求视为失败
        for (_, tx) in pending.lock().unwrap().drain() {
            let _ = tx.send(Err(RpcError::new(
                vine_plugin_api::codes::PLUGIN_INTERNAL,
                "插件 stdout 已关闭",
            )));
        }
    });
}

fn spawn_stderr_thread(stderr: std::process::ChildStderr, log: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let mut log = log.lock().unwrap();
            // 简单环形上限，防无限增长
            if log.len() >= 1000 {
                log.remove(0);
            }
            log.push(line);
        }
    });
}
