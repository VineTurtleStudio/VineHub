//! VineHub 宿主核心：插件注册表、Sidecar 进程管理、配置存储。

pub mod registry;

pub use registry::{PluginEntry, PluginRegistry, PluginStatus, RegistryError, ScanSummary};

pub const HOST_APP_NAME: &str = "VineHub";
