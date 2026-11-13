use serde::{Deserialize, Serialize};

// ---------- 配置 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeletePolicy {
    Versioned,
    Mirror,
    AddOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriggerMode {
    Manual,
    Interval,
    Realtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TriggerConfig {
    pub mode: TriggerMode,
    pub interval_minutes: u32,
    pub debounce_ms: u64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            mode: TriggerMode::Interval,
            interval_minutes: 10,
            debounce_ms: 3000,
        }
    }
}

fn new_pair_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AfterSyncAction {
    #[default]
    None,
    Script,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PairConfig {
    pub id: String,
    pub name: String,
    pub source: String,
    pub dest: String,
    pub enabled: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    /// 定时同步间隔覆盖（分钟，仅在全局触发模式为「按间隔」时生效；None = 使用全局间隔）
    pub interval_minutes: Option<u32>,
}

impl Default for PairConfig {
    fn default() -> Self {
        Self {
            id: new_pair_id(),
            name: String::new(),
            source: String::new(),
            dest: String::new(),
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            interval_minutes: None,
        }
    }
}

/// 目标是盘符根目录而源不是整盘 → 高风险配置：
/// 目标盘根下所有既有文件都会落入删除/版本备份计划（如目标 `F:\` 会波及整盘）。
/// 保存校验与预览/同步运行时都会拒绝；源与目标都是盘符根（整盘对整盘）则放行。
pub fn dest_root_unsafe(pair: &PairConfig) -> bool {
    crate::core::is_drive_root(&pair.dest) && !crate::core::is_drive_root(&pair.source)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    #[default]
    Zh,
    En,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub pairs: Vec<PairConfig>,
    pub delete_policy: DeletePolicy,
    pub trigger: TriggerConfig,
    pub hash_verify: bool,
    pub notify_on_finish: bool,
    pub confirm_before_sync: bool,
    pub version_keep_days: u32,
    pub version_on_replace: bool,
    pub version_on_delete: bool,
    pub version_max_count: u32,
    pub copy_threads: u32,
    pub theme: Theme,
    pub language: Language,
    pub autostart: bool,
    /// 同步全部成功后的附加动作
    pub after_sync_action: AfterSyncAction,
    /// 动作为「执行脚本」时的脚本路径（.ps1 / .bat / .cmd / 可执行文件）
    pub after_sync_script: String,
    /// 版本备份文件名中的时间戳格式（chrono 格式串）
    pub version_timestamp_format: String,
    /// 复制完成后回读目标并与源比对哈希（更慢但可发现写入损坏）
    pub verify_after_copy: bool,
    /// 快速扫描：目标端状态以上次成功同步的 file_state 快照推导，跳过目标目录遍历
    pub diff_scan: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pairs: Vec::new(),
            delete_policy: DeletePolicy::Versioned,
            trigger: TriggerConfig::default(),
            hash_verify: false,
            notify_on_finish: true,
            confirm_before_sync: true,
            version_keep_days: 30,
            version_on_replace: true,
            version_on_delete: true,
            version_max_count: 0,
            copy_threads: 4,
            theme: Theme::System,
            language: Language::Zh,
            autostart: false,
            after_sync_action: AfterSyncAction::None,
            after_sync_script: String::new(),
            version_timestamp_format: "%Y-%m-%d_%H-%M-%S".to_string(),
            verify_after_copy: false,
            diff_scan: false,
        }
    }
}

// ---------- 前端 DTO ----------

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub added: i64,
    pub updated: i64,
    pub deleted: i64,
    pub versioned: i64,
    pub skipped: i64,
    pub failed: i64,
}

impl Counts {
    pub fn total_changes(&self) -> i64 {
        self.added + self.updated + self.deleted + self.versioned
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStateDto {
    pub pair_id: String,
    pub pair_name: String,
    pub last_sync_at: Option<String>,
    pub last_status: Option<String>,
    pub added: i64,
    pub updated: i64,
    pub deleted: i64,
    pub versioned: i64,
    pub failed: i64,
    pub message: String,
}

impl Default for PairStateDto {
    fn default() -> Self {
        Self {
            pair_id: String::new(),
            pair_name: String::new(),
            last_sync_at: None,
            last_status: None,
            added: 0,
            updated: 0,
            deleted: 0,
            versioned: 0,
            failed: 0,
            message: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateDto {
    pub config: Config,
    pub pair_states: Vec<PairStateDto>,
    pub syncing: bool,
    pub queued: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Add,
    Update,
    Delete,
    Versioned,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDto {
    pub pair_id: String,
    pub pair_name: String,
    pub kind: ChangeKind,
    pub rel_path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDto {
    pub changes: Vec<ChangeDto>,
    pub counts: Counts,
    pub total_bytes: u64,
    pub suppressed: i64,
    pub target_unavailable: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRow {
    pub id: i64,
    pub pair_id: String,
    pub pair_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub added: i64,
    pub updated: i64,
    pub deleted: i64,
    pub versioned: i64,
    pub skipped: i64,
    pub failed: i64,
    pub trigger: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub pair_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub pair_id: Option<String>,
    pub trigger: Option<String>,
}

/// 过滤器测试结果（前端展示样例路径是否命中）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterTestDto {
    pub matched: bool,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRequest {
    pub pair_id: Option<String>,
}

// ---------- 事件负载 ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedEvt {
    pub trigger: String,
    pub pairs: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvt {
    pub pair_name: String,
    pub rel_path: String,
    pub action: ChangeKind,
    pub done: usize,
    pub total: usize,
    /// 已完成变更的累计传输字节数（仅新增/更新计入），供前端计算速度
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishedEvt {
    pub ok: bool,
    pub trigger: String,
    pub pairs: usize,
    pub counts: Counts,
    pub failures: usize,
    pub target_unavailable: bool,
    pub duration_ms: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressEvt {
    pub pair_name: String,
    /// "source" | "target" | "hash"
    pub phase: String,
    pub files: u64,
    /// 该阶段总量（0 = 未知，扫描阶段即如此）
    pub total: u64,
    pub rel_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfoDto {
    pub portable: bool,
    pub data_dir: String,
    pub exe_path: String,
    pub version: String,
}
