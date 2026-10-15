pub mod dispatch;
pub mod entropy;
pub mod hash;
pub mod stream;
pub mod walker;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Serialize, Serializer};

/// 结论等级：Ok（正常）/ Warning（可疑）/ Corrupt（确认损坏）/ Unknown（无法判定）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warning,
    Corrupt,
    Unknown,
}

impl Severity {
    pub fn rank(self) -> u8 {
        match self {
            Severity::Ok => 0,
            Severity::Warning => 1,
            Severity::Unknown => 2,
            Severity::Corrupt => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Severity::Ok => "正常",
            Severity::Warning => "可疑",
            Severity::Corrupt => "损坏",
            Severity::Unknown => "未知",
        }
    }

    pub fn is_problem(self) -> bool {
        self != Severity::Ok
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 单个问题：结论 + 问题编码 + 中文描述 + 字节偏移
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub offset: Option<u64>,
}

impl Finding {
    pub fn new(
        severity: Severity,
        code: &'static str,
        message: impl Into<String>,
        offset: Option<u64>,
    ) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            offset,
        }
    }

    pub fn ok(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Ok, code, message, None)
    }

    pub fn warning(code: &'static str, message: impl Into<String>, offset: Option<u64>) -> Self {
        Self::new(Severity::Warning, code, message, offset)
    }

    pub fn corrupt(code: &'static str, message: impl Into<String>, offset: Option<u64>) -> Self {
        Self::new(Severity::Corrupt, code, message, offset)
    }

    pub fn unknown(code: &'static str, message: impl Into<String>, offset: Option<u64>) -> Self {
        Self::new(Severity::Unknown, code, message, offset)
    }
}

/// 检测模式：盲检 / 哈希清单比对
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Blind,
    Hash,
}

/// 单个文件的检测报告
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub path: PathBuf,
    pub file_type: String,
    /// 文件大小（字节）。由目录遍历阶段的 metadata 得到，避免扫描中重复 stat。
    pub size: u64,
    pub mode: Mode,
    pub findings: Vec<Finding>,
    #[serde(serialize_with = "ser_duration_ms")]
    pub duration: Duration,
}

fn ser_duration_ms<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_millis() as u64)
}

impl Report {
    pub fn new(path: PathBuf, file_type: impl Into<String>, mode: Mode) -> Self {
        Self {
            path,
            file_type: file_type.into(),
            size: 0,
            mode,
            findings: Vec::new(),
            duration: Duration::ZERO,
        }
    }

    pub fn push(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub fn extend(&mut self, findings: impl IntoIterator<Item = Finding>) {
        self.findings.extend(findings);
    }

    pub fn worst(&self) -> Severity {
        self.findings
            .iter()
            .map(|f| f.severity)
            .max()
            .unwrap_or(Severity::Ok)
    }

    pub fn problem_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity.is_problem())
            .count()
    }

    pub fn is_ok(&self) -> bool {
        self.worst() == Severity::Ok
    }

    /// 「首因」：严重度最高的问题，供结果表徽章与悬停摘要使用。
    ///
    /// 返回的不是 Ok 级提示（那类信息价值低，留给详情面板）；无问题返回 None。
    /// 同级多个时取最靠前的一个，行为稳定可预测。
    pub fn primary_finding(&self) -> Option<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity.is_problem())
            .max_by_key(|f| f.severity)
    }
}

/// 全局检测选项
#[derive(Debug, Clone)]
pub struct Opts {
    /// 强制对超过 `max_deep_size` 的文件也做深度解码
    pub deep: bool,
    /// 小于等于该大小的文件总是做深度解码（字节）
    pub max_deep_size: u64,
    /// 采样熵低于该值时判定为低熵/全零填充
    pub entropy_threshold: f64,
    /// 自动寻找同名 sidecar 哈希清单（如 a.zip.sha256）
    pub sidecar_hash: bool,
    /// 哈希清单模式索引：路径 →（算法, 期望哈希）
    pub manifest_index:
        Option<std::sync::Arc<std::collections::HashMap<PathBuf, (hash::HashKind, String)>>>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            deep: false,
            max_deep_size: 64 * 1024 * 1024,
            entropy_threshold: 0.5,
            sidecar_hash: true,
            manifest_index: None,
        }
    }
}

impl Opts {
    pub fn should_deep(&self, size: u64) -> bool {
        self.deep || size <= self.max_deep_size
    }

    /// 核心完整性校验是否执行（对照 [`FULL_CHECK_LIMIT`]）
    pub fn core_deep(&self, size: u64) -> bool {
        size <= FULL_CHECK_LIMIT || self.should_deep(size)
    }
}

/// 核心完整性校验（逐条解压 CRC、gzip 尾部 CRC、SQLite quick_check、FLAC 全量 MD5 等）
/// 的默认大小上限：**以内一律执行**，不受 `max_deep_size` 限制；超过时默认记 Ok 级说明，
/// 勾选「强制深度解码」(-d) 或调高 `max_deep_size` 后照常执行（一律流式、内存恒定）。
pub const FULL_CHECK_LIMIT: u64 = 512 * 1024 * 1024;

/// 分发器读取的文件摘要
#[derive(Debug, Clone)]
pub struct FileHint {
    pub path: PathBuf,
    /// 小写扩展名（不含点）
    pub ext: String,
    pub size: u64,
    /// 魔数识别出的真实类型
    pub magic: Option<dispatch::MagicKind>,
    /// 文件头部字节（最多 512）
    pub head: Vec<u8>,
}

impl FileHint {
    pub fn probe(path: &Path) -> std::io::Result<Self> {
        use std::io::Read;

        let meta = std::fs::metadata(path)?;
        let mut file = std::fs::File::open(path)?;
        // ID3v2 标签可能远超默认窗口（实测有 4 KiB+ 的），真实媒体签名（fLaC 等）在标签之后。
        // 先读 512B 探测到 ID3 头，再按其声明长度把窗口扩到签名之后，避免带标签的 FLAC 被误判为 MP3。
        let mut head = Vec::with_capacity(512);
        let _ = (&mut file).take(512).read_to_end(&mut head)?;
        if head.starts_with(b"ID3")
            && let Some(tag_end) = dispatch::id3_tag_len(&head)
            && tag_end > head.len()
        {
            // 上限 1 MiB：再大的标签也只在头 1 MiB 内找签名
            let want = tag_end.saturating_add(64).min(1024 * 1024);
            let _ = (&mut file)
                .take((want - head.len()) as u64)
                .read_to_end(&mut head)?;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let magic = dispatch::detect_magic(&head);
        Ok(Self {
            path: path.to_path_buf(),
            ext,
            size: meta.len(),
            magic,
            head,
        })
    }

    /// 报告中展示的类型：优先魔数，其次按扩展名推断
    pub fn type_name(&self) -> String {
        if let Some(kind) = self.magic {
            return kind.name().to_string();
        }
        if self.size == 0 {
            return "empty".to_string();
        }
        if crate::verifiers::generic::text_ext(&self.ext) {
            return "text".to_string();
        }
        "unknown".to_string()
    }
}

/// 验证器接口：按提示选择（supports），返回该格式的检测报告（verify）
pub trait Verifier: Send + Sync {
    fn supports(&self, hint: &FileHint) -> bool;
    fn verify(&self, path: &Path, opts: &Opts) -> Report;
}
