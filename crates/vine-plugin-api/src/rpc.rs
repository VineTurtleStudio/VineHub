//! RPC 协议 v1：JSON-RPC 2.0 风格，一行一条 UTF-8 JSON。
//!
//! - 请求带 `id`，响应必须回相同 `id`（配对 = 终态）
//! - 通知（如 `task.progress`、`task.cancel`）不带 `id`
//! - stderr 一律视为日志，不参与协议

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codes;

/// 规范 v1 中的固定方法名。
pub mod method {
    /// 连接后宿主必发握手。
    pub const PLUGIN_INFO: &str = "plugin.info";
    /// 心跳（宿主每 15s）。
    pub const PING: &str = "ping";
    /// vinePulse 扫描启动。
    pub const SCAN_START: &str = "scan.start";
    /// vinePress 试算分类（不落盘）。
    pub const PLAN_START: &str = "plan.start";
    /// vinePress 查找重复文件。
    pub const DUPES_FIND: &str = "dupes.find";
    /// vinePress 执行导出（试算结果 → 压缩包）。
    pub const EXPORT_START: &str = "export.start";
    /// vineSync 试算同步差异（不落盘）。
    pub const SYNC_PREVIEW: &str = "sync.preview";
    /// vineSync 执行同步。
    pub const SYNC_START: &str = "sync.start";
    /// vineCurator 建库根目录（headless 注册）。
    pub const LIBRARY_ADD_ROOT: &str = "library.addRoot";
    /// vineCurator 列出已注册根目录。
    pub const LIBRARY_ROOTS: &str = "library.roots";
    /// vineCurator 扫描根目录入库。
    pub const LIBRARY_SCAN: &str = "library.scan";
    /// vineCurator 全文检索。
    pub const SEARCH_ITEMS: &str = "search.items";
    /// 取消任务（通知）。
    pub const TASK_CANCEL: &str = "task.cancel";
    /// 进度通知。
    pub const TASK_PROGRESS: &str = "task.progress";
}

/// 请求 id：数字或字符串。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl From<i64> for RequestId {
    fn from(v: i64) -> Self {
        Self::Number(v)
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(s) => write!(f, "{s}"),
        }
    }
}

/// 请求消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl RpcRequest {
    /// 构造请求。
    pub fn new(id: impl Into<Option<RequestId>>, method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            method: method.into(),
            params: None,
        }
    }

    /// 附带参数。
    pub fn with_params(mut self, params: impl Serialize) -> Result<Self, RpcMessageError> {
        self.params = Some(serde_json::to_value(params).map_err(|e| RpcMessageError {
            code: codes::INVALID_PARAMS,
            message: e.to_string(),
        })?);
        Ok(self)
    }

    /// 通知（无 id）。
    pub fn notification(method: impl Into<String>) -> Self {
        Self::new(None, method)
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// 错误对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// 响应消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// 成功响应（必带 id 配对）。
    pub fn success(id: RequestId, result: impl Serialize) -> Result<Self, RpcMessageError> {
        let result = serde_json::to_value(result).map_err(|e| RpcMessageError {
            code: codes::PLUGIN_INTERNAL,
            message: e.to_string(),
        })?;
        Ok(Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
        })
    }

    /// 错误响应。
    pub fn error(id: Option<RequestId>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// 行解码 / 编码错误（可映射为错误码响应）。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct RpcMessageError {
    pub code: i32,
    pub message: String,
}

impl RpcMessageError {
    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            code: codes::PARSE_ERROR,
            message: message.into(),
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: codes::INVALID_REQUEST,
            message: message.into(),
        }
    }

    /// 转为协议错误对象。
    pub fn into_rpc_error(self) -> RpcError {
        RpcError::new(self.code, self.message)
    }
}

/// 序列化为单行 JSON（禁止内嵌换行）。
pub fn to_line<T: Serialize>(value: &T) -> Result<String, RpcMessageError> {
    let line = serde_json::to_string(value).map_err(|e| RpcMessageError {
        code: codes::PLUGIN_INTERNAL,
        message: e.to_string(),
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(RpcMessageError::invalid_request(
            "编码结果包含换行，违反行分隔协议",
        ));
    }
    Ok(line)
}

/// 解析一行输入为请求。
pub fn parse_request_line(line: &str) -> Result<RpcRequest, RpcMessageError> {
    let value: Value = serde_json::from_str(line).map_err(|e| RpcMessageError::parse(e.to_string()))?;
    let req: RpcRequest =
        serde_json::from_value(value).map_err(|e| RpcMessageError::invalid_request(e.to_string()))?;
    if req.jsonrpc != "2.0" {
        return Err(RpcMessageError::invalid_request(format!(
            "jsonrpc 版本必须为 \"2.0\"，得到 {:?}",
            req.jsonrpc
        )));
    }
    if req.method.trim().is_empty() {
        return Err(RpcMessageError::invalid_request("method 不能为空"));
    }
    Ok(req)
}

/// 解析一行输入为响应。
pub fn parse_response_line(line: &str) -> Result<RpcResponse, RpcMessageError> {
    let value: Value = serde_json::from_str(line).map_err(|e| RpcMessageError::parse(e.to_string()))?;
    let resp: RpcResponse =
        serde_json::from_value(value).map_err(|e| RpcMessageError::invalid_request(e.to_string()))?;
    if resp.jsonrpc != "2.0" {
        return Err(RpcMessageError::invalid_request(format!(
            "jsonrpc 版本必须为 \"2.0\"，得到 {:?}",
            resp.jsonrpc
        )));
    }
    if resp.result.is_some() && resp.error.is_some() {
        return Err(RpcMessageError::invalid_request(
            "result 与 error 不得同时出现",
        ));
    }
    // 纯通知行（无 id 且无 result/error）不是响应
    if resp.id.is_none() && resp.result.is_none() && resp.error.is_none() {
        return Err(RpcMessageError::invalid_request(
            "缺少 id 与 result/error，不是响应消息",
        ));
    }
    Ok(resp)
}

// ---------- 规范 v1 载荷 DTO ----------

/// `plugin.info` 的 result 载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub version: String,
    pub api_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// `scan.start` 参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanStartParams {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    /// 强制深度解码（超过大小阈值也解码）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep: Option<bool>,
}

/// `scan.start` 的即时 result（终态响应另带 ok/bad/report）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStartResult {
    pub task_id: String,
}

/// `task.progress` 通知参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressParams {
    pub task_id: String,
    pub done: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
}

/// `task.cancel` 参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCancelParams {
    pub task_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_with_id() {
        let req = RpcRequest::new(RequestId::Number(0), method::PLUGIN_INFO);
        let line = to_line(&req).unwrap();
        assert!(!line.contains('\n'));
        let parsed = parse_request_line(&line).unwrap();
        assert_eq!(parsed, req);
        assert!(!parsed.is_notification());
    }

    #[test]
    fn notification_has_no_id() {
        let note = RpcRequest::notification(method::TASK_PROGRESS).with_params(TaskProgressParams {
            task_id: "t-42".into(),
            done: 1,
            total: 10,
            msg: Some("开始".into()),
        }).unwrap();
        let line = to_line(&note).unwrap();
        let parsed = parse_request_line(&line).unwrap();
        assert!(parsed.is_notification());
    }

    #[test]
    fn response_success_and_error() {
        let ok = RpcResponse::success(RequestId::Number(1), ScanStartResult { task_id: "t-1".into() }).unwrap();
        let parsed = parse_response_line(&to_line(&ok).unwrap()).unwrap();
        assert!(parsed.is_ok());
        assert_eq!(parsed.id, Some(RequestId::Number(1)));

        let err = RpcResponse::error(Some(RequestId::Number(2)), RpcError::new(codes::CANCELLED, "已取消"));
        let parsed = parse_response_line(&to_line(&err).unwrap()).unwrap();
        assert!(!parsed.is_ok());
        assert_eq!(parsed.error.unwrap().code, codes::CANCELLED);
    }

    #[test]
    fn parse_errors_map_to_codes() {
        assert_eq!(parse_request_line("{").unwrap_err().code, codes::PARSE_ERROR);
        assert_eq!(
            parse_request_line(r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#).unwrap_err().code,
            codes::INVALID_REQUEST
        );
        assert_eq!(
            parse_request_line(r#"{"jsonrpc":"2.0","id":1,"method":"  "}"#).unwrap_err().code,
            codes::INVALID_REQUEST
        );
        assert_eq!(
            parse_request_line(r#"{"jsonrpc":"2.0","result":1,"error":{"code":1,"message":"x"}}"#)
                .unwrap_err()
                .code,
            codes::INVALID_REQUEST
        );
    }

    #[test]
    fn notification_line_is_not_a_response() {
        let note = r#"{"jsonrpc":"2.0","method":"task.progress","params":{"taskId":"t"}}"#;
        let err = parse_response_line(note).unwrap_err();
        assert_eq!(err.code, codes::INVALID_REQUEST);
        // 但可以按请求/通知解析
        assert!(parse_request_line(note).unwrap().is_notification());
    }

    #[test]
    fn error_codes_are_distinct() {
        let all = [
            codes::PARSE_ERROR,
            codes::INVALID_REQUEST,
            codes::METHOD_NOT_FOUND,
            codes::INVALID_PARAMS,
            codes::CANCELLED,
            codes::TIMEOUT,
            codes::PERMISSION_DENIED,
            codes::TASK_NOT_FOUND,
            codes::PLUGIN_INTERNAL,
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
    }

    #[test]
    fn plugin_info_camel_case_wire_format() {
        let info = PluginInfo {
            id: "vine.pulse".into(),
            version: "0.1.0".into(),
            api_version: "1.0".into(),
            capabilities: vec!["scan".into(), "report".into()],
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["apiVersion"], "1.0");
        assert_eq!(value["capabilities"].as_array().unwrap().len(), 2);
    }
}
