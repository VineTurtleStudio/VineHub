//! 标准 JSON-RPC 错误码与 VineHub 应用错误码。

/// JSON 解析失败。
pub const PARSE_ERROR: i32 = -32700;
/// 请求不是合法 JSON-RPC 消息。
pub const INVALID_REQUEST: i32 = -32600;
/// 方法不存在。
pub const METHOD_NOT_FOUND: i32 = -32601;
/// 参数非法。
pub const INVALID_PARAMS: i32 = -32602;

/// 任务已取消。
pub const CANCELLED: i32 = -32001;
/// 操作超时。
pub const TIMEOUT: i32 = -32002;
/// 权限拒绝。
pub const PERMISSION_DENIED: i32 = -32003;
/// 任务不存在。
pub const TASK_NOT_FOUND: i32 = -32004;
/// 插件内部错误。
pub const PLUGIN_INTERNAL: i32 = -32005;
/// 面板桥接：方法不在白名单。
pub const BRIDGE_METHOD_NOT_ALLOWED: i32 = -32010;

/// 错误码 → 人类可读名称。
pub fn code_name(code: i32) -> &'static str {
    match code {
        PARSE_ERROR => "ParseError",
        INVALID_REQUEST => "InvalidRequest",
        METHOD_NOT_FOUND => "MethodNotFound",
        INVALID_PARAMS => "InvalidParams",
        CANCELLED => "Cancelled",
        TIMEOUT => "Timeout",
        PERMISSION_DENIED => "PermissionDenied",
        TASK_NOT_FOUND => "TaskNotFound",
        PLUGIN_INTERNAL => "PluginInternal",
        BRIDGE_METHOD_NOT_ALLOWED => "BridgeMethodNotAllowed",
        _ => "Unknown",
    }
}
