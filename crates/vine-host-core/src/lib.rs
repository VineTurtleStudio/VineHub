//! VineHub 宿主核心：插件注册表、Sidecar 进程管理、配置存储。

pub mod registry;
pub mod sidecar;
pub mod storage;

pub use registry::{PluginEntry, PluginRegistry, PluginStatus, RegistryError, ScanSummary};
pub use sidecar::{SidecarConfig, SidecarError, SidecarHandle, SidecarManager};
pub use storage::{PluginRecord, Store, StorageError, TaskRecord, TaskStatus};

pub const HOST_APP_NAME: &str = "VineHub";
