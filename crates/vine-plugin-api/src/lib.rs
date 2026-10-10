//! VineHub 插件规范 API。
//!
//! 包含三部分：
//! - `manifest`：`plugin.json` 结构与校验规则
//! - `rpc`：JSON-RPC 2.0 风格的行分隔消息类型与编解码
//! - `codes`：标准 / 应用错误码

pub mod codes;
pub mod manifest;
pub mod rpc;
pub mod version;

pub use codes::*;
pub use manifest::{
    CommandSpec, FsMode, ManifestError, PanelSpec, Permissions, PluginKind, PluginManifest,
};
pub use rpc::{
    method, parse_request_line, parse_response_line, to_line, PluginInfo, RequestId, RpcError,
    RpcMessageError, RpcRequest, RpcResponse, ScanStartParams, ScanStartResult, TaskCancelParams,
    TaskProgressParams,
};
pub use version::{ApiVersion, VersionError};

/// 宿主当前实现的插件 API 版本。
pub const HOST_API_VERSION: &str = "1.0";

/// 第一方插件清单文件名。
pub const PLUGIN_MANIFEST_FILENAME: &str = "plugin.json";
