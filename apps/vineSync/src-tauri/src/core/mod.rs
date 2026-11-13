pub mod copy;
pub mod filter;
pub mod scan;
pub mod version;

use crate::model::{ChangeKind, DeletePolicy};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// 同步引擎内部使用的文件元数据
#[derive(Debug, Clone)]
pub struct FileMeta {
    /// 相对路径（保留原始大小写与分隔符）
    pub rel: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// 一条变更记录
#[derive(Debug, Clone)]
pub struct Change {
    pub kind: ChangeKind,
    /// 展示用相对路径
    pub rel: String,
    pub size: u64,
    /// 源相对路径（新增/更新）
    pub src_rel: Option<String>,
    /// 目标相对路径（更新/删除/版本备份）
    pub dst_rel: Option<String>,
}

pub type E<T> = Result<T, String>;

pub fn systemtime_to_ns(t: std::time::SystemTime) -> i64 {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos().min(i64::MAX as u128) as i64,
        Err(e) => -(e.duration().as_nanos().min(i64::MAX as u128) as i64),
    }
}

pub fn ns_to_systemtime(ns: i64) -> std::time::SystemTime {
    if ns >= 0 {
        std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns as u64)
    } else {
        std::time::UNIX_EPOCH - std::time::Duration::from_nanos((-ns) as u64)
    }
}

/// 规范化路径字符串用于比较：'/' 分隔、去尾部分隔符、小写
pub fn normalize_key(rel: &str) -> String {
    rel.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// 目标路径是否可用（如 "F:\备份" → 检查 "F:\"，"\\nas\share\x" → 检查 "\\nas\share"）。
///
/// 网络路径（UNC / 映射盘）在服务器不可达时可能阻塞数十秒，
/// 因此实际探测在独立线程中执行，超时视为不可用，避免卡住同步队列。
pub fn drive_available(path: &str) -> bool {
    drive_available_timeout(path, 8)
}

pub fn drive_available_timeout(path: &str, timeout_secs: u64) -> bool {
    let target = probe_root(path);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Path::new(&target).exists());
    });
    rx.recv_timeout(std::time::Duration::from_secs(timeout_secs))
        .unwrap_or(false)
}

/// 取实际探测的根路径：盘符 → `X:\`；UNC → `\\server\share`；其余原样
fn probe_root(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return format!("{}\\", &path[..2]);
    }
    if let Some(rest) = path.strip_prefix("\\\\") {
        let mut parts = rest.split('\\').filter(|s| !s.is_empty());
        if let (Some(host), Some(share)) = (parts.next(), parts.next()) {
            return format!("\\\\{host}\\{share}");
        }
    }
    path.to_string()
}

/// 路径字符串规范化（用于校验比较）
pub fn normalize_path_str(p: &str) -> String {
    let mut s = p.trim().replace('/', "\\");
    while s.contains("\\\\") && !s.starts_with("\\\\") {
        s = s.replace("\\\\", "\\");
    }
    while s.ends_with('\\') && s.len() > 1 {
        s.pop();
    }
    s.to_lowercase()
}

/// 是否为盘符根目录（`F:\`、`f:/`、`F:` 均算；UNC 共享根不算）
pub fn is_drive_root(path: &str) -> bool {
    let s = normalize_path_str(path);
    s.len() == 2 && s.ends_with(':') && s.as_bytes()[0].is_ascii_alphabetic()
}

/// Windows 超长路径（>260，MAX_PATH）：把绝对路径转为 verbatim 形式
/// （`D:\x` → `\\?\D:\x`，`\\server\share` → `\\?\UNC\server\share`），
/// 绕过 260 字符限制。相对路径与已带 `\\?\` 前缀的路径原样返回。
///
/// 同步流程在根路径上调用一次，子路径经 `join` 自动继承前缀；
/// 不做长度判断（短根下的深层子目录同样依赖前缀）。
pub fn to_long_path(p: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::path::PathBuf;

        let wide: Vec<u16> = p.as_os_str().encode_wide().collect();
        const VERBATIM: [u16; 4] = [92, 92, 63, 92]; // \\?\
        let verbatim = wide.len() >= 4 && wide[..4] == VERBATIM;
        if !p.is_absolute() || verbatim {
            return p.to_path_buf();
        }
        let unc = wide.len() >= 2 && wide[0] == 92 && wide[1] == 92;
        let mut out: Vec<u16> = Vec::with_capacity(wide.len() + 8);
        out.extend(r"\\?\".encode_utf16());
        if unc {
            out.extend(r"UNC\".encode_utf16());
            out.extend_from_slice(&wide[2..]);
        } else {
            out.extend_from_slice(&wide);
        }
        PathBuf::from(OsString::from_wide(&out))
    }
    #[cfg(not(windows))]
    {
        p.to_path_buf()
    }
}

/// 一对文件夹的对比结果
#[derive(Debug, Default)]
pub struct DiffResult {
    pub changes: Vec<Change>,
    /// 被删除策略抑制的删除数量
    pub suppressed: i64,
}

/// 对比源与目标快照，生成变更计划
/// `version_on_delete`：删除策略 A 下源删除的文件是否移入版本备份（false = 直接删除）
/// `hash_differs`：哈希校验阶段已判定「内容不同」的规范化 key 集合
/// （大小与 mtime 都相同才进入哈希比对；比对本身由 runtime 并行执行）
pub fn diff(
    src: &BTreeMap<String, FileMeta>,
    dst: &BTreeMap<String, FileMeta>,
    policy: DeletePolicy,
    version_on_delete: bool,
    hash_differs: &HashSet<String>,
) -> DiffResult {
    let mut out = DiffResult::default();

    for (key, s) in src {
        match dst.get(key) {
            None => out.changes.push(Change {
                kind: ChangeKind::Add,
                rel: s.rel.clone(),
                size: s.size,
                src_rel: Some(s.rel.clone()),
                dst_rel: None,
            }),
            Some(d) => {
                let need = s.size != d.size
                    || s.mtime_ns != d.mtime_ns
                    || hash_differs.contains(key);
                if need {
                    out.changes.push(Change {
                        kind: ChangeKind::Update,
                        rel: s.rel.clone(),
                        size: s.size,
                        src_rel: Some(s.rel.clone()),
                        dst_rel: Some(d.rel.clone()),
                    });
                }
            }
        }
    }

    for (key, d) in dst {
        if src.contains_key(key) {
            continue;
        }
        match policy {
            DeletePolicy::Mirror => out.changes.push(Change {
                kind: ChangeKind::Delete,
                rel: d.rel.clone(),
                size: d.size,
                src_rel: None,
                dst_rel: Some(d.rel.clone()),
            }),
            DeletePolicy::Versioned if version_on_delete => out.changes.push(Change {
                kind: ChangeKind::Versioned,
                rel: d.rel.clone(),
                size: d.size,
                src_rel: None,
                dst_rel: Some(d.rel.clone()),
            }),
            // 关闭「删除时备份」后，策略 A 的删除退化为直接删除
            DeletePolicy::Versioned => out.changes.push(Change {
                kind: ChangeKind::Delete,
                rel: d.rel.clone(),
                size: d.size,
                src_rel: None,
                dst_rel: Some(d.rel.clone()),
            }),
            DeletePolicy::AddOnly => out.suppressed += 1,
        }
    }

    out
}

/// 流式计算文件 SHA-256（十六进制）
pub fn file_hash(path: &Path) -> E<String> {
    Ok(hex(&file_digest(path)?))
}

fn hex(digest: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 流式计算文件 SHA-256 原始摘要（32 字节，避免逐文件生成十六进制串）
pub fn file_digest(path: &Path) -> E<[u8; 32]> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let file = std::fs::File::open(path)
        .map_err(|e| crate::msg!("打开文件失败 {}: {e}", "Failed to open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = (&file)
            .read(&mut buf)
            .map_err(|e| crate::msg!("读取失败 {}: {e}", "Read failed {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

// ---------- 摘要缓存 ----------
//
// 「预览 → 用户确认 → 同步」是同一次同步的两个阶段，两阶段都会对大目录做
// 哈希比对。没有缓存时每个文件要被读 4 遍（源/目标 × 2 阶段），几十万文件
// 就是几十分钟纯 I/O。这里按 (路径, 大小, mtime) 缓存摘要，第二阶段直接命中；
// 同步写入完成后再整体失效，保证下一次同步不会用到过期摘要。

struct HashEntry {
    size: u64,
    mtime_ns: i64,
    digest: [u8; 32],
    at: std::time::Instant,
}

const HASH_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(600);
const HASH_CACHE_MAX: usize = 100_000;

fn hash_cache() -> &'static std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, HashEntry>>
{
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, HashEntry>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 带缓存的摘要：`size`/`mtime_ns` 与缓存不一致或过期时重新计算。
pub fn file_digest_cached(path: &Path, size: u64, mtime_ns: i64) -> E<[u8; 32]> {
    let hit = {
        let cache = hash_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.get(path).and_then(|e| {
            (e.size == size && e.mtime_ns == mtime_ns && e.at.elapsed() < HASH_CACHE_TTL)
                .then_some(e.digest)
        })
    };
    if let Some(d) = hit {
        return Ok(d);
    }
    let digest = file_digest(path)?;
    let mut cache = hash_cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= HASH_CACHE_MAX {
        cache.clear();
    }
    cache.insert(
        path.to_path_buf(),
        HashEntry { size, mtime_ns, digest, at: std::time::Instant::now() },
    );
    Ok(digest)
}

/// 同步写入后调用：目标内容已改变，缓存摘要立即失效
pub fn clear_hash_cache() {
    hash_cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_root_for_drive_letter() {
        assert_eq!(probe_root("F:\\备份"), "F:\\");
        assert_eq!(probe_root("C:"), "C:\\");
        assert_eq!(probe_root("C:/docs"), "C:\\");
    }

    #[test]
    fn probe_root_for_unc() {
        assert_eq!(
            probe_root("\\\\nas\\share\\文档\\a.txt"),
            "\\\\nas\\share"
        );
        assert_eq!(probe_root("\\\\nas\\share"), "\\\\nas\\share");
        // 不完整 UNC 原样返回
        assert_eq!(probe_root("\\\\nas"), "\\\\nas");
    }

    #[test]
    fn drive_available_local_paths() {
        let tmp = std::env::temp_dir();
        // 本地盘符路径探测盘根（存在）
        assert!(drive_available(&tmp.to_string_lossy()));
        // 盘符计算与 probe_root 一致
        assert_eq!(probe_root(&tmp.to_string_lossy()).len(), 3);
    }

    #[test]
    fn hash_missing_file_is_error() {
        assert!(file_hash(Path::new("Z:\\__no_such__\\x.txt")).is_err());
    }

    #[test]
    fn is_drive_root_cases() {
        assert!(is_drive_root("F:\\"));
        assert!(is_drive_root("f:/"));
        assert!(is_drive_root("F:"));
        assert!(is_drive_root("  D:\\  "));
        assert!(!is_drive_root("F:\\备份"));
        assert!(!is_drive_root("F:\\备份\\"));
        assert!(!is_drive_root("\\\\nas\\share"));
        assert!(!is_drive_root(""));
        assert!(!is_drive_root("目录"));
    }

    #[test]
    fn to_long_path_cases() {
        use std::path::PathBuf;
        // 相对路径原样
        assert_eq!(to_long_path(Path::new("a\\b")), PathBuf::from("a\\b"));
        // 绝对路径（无论长短）加 verbatim 前缀——短根下的深层子目录同样依赖前缀
        assert_eq!(to_long_path(Path::new("D:\\docs")), PathBuf::from(r"\\?\D:\docs"));
        assert_eq!(to_long_path(Path::new("D:\\")), PathBuf::from(r"\\?\D:\"));
        // 已是 verbatim 不重复叠加
        assert_eq!(to_long_path(Path::new(r"\\?\D:\x")), PathBuf::from(r"\\?\D:\x"));
        // UNC → \\?\UNC\
        assert_eq!(
            to_long_path(Path::new(r"\\nas\share\x")),
            PathBuf::from(r"\\?\UNC\nas\share\x")
        );
        // 转换后可直接用于文件系统操作
        let tmp = std::env::temp_dir();
        assert!(to_long_path(&tmp).exists());
    }
}
