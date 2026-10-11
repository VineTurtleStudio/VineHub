//! 导出清单（`<输出根>/vinepress-manifest.json`）与完整性校验（纯逻辑，零 Tauri 依赖）
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MANIFEST_NAME: &str = "vinepress-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// 单位条目相对根路径（正斜杠）
    pub rel_path: String,
    /// 相对输出根的压缩包路径（正斜杠）
    pub archive: String,
    pub size: u64,
    /// 文件修改时间（毫秒），用于跳过未变动文件的重复哈希
    pub mtime_ms: u64,
    /// 十六进制小写 SHA-256
    pub sha256: String,
    pub written_at: u64,
    /// 单位增量指纹（源目录统计 + 相关设置 + 说明正文），创建时写入；
    /// 空串表示未知/已失效（不可增量跳过）。旧清单缺省即为空，安全降级为全量导出。
    #[serde(default)]
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub updated_at: u64,
    pub files: Vec<ManifestFile>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            updated_at: now(),
            files: Vec::new(),
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn manifest_path(output_root: &Path) -> PathBuf {
    output_root.join(MANIFEST_NAME)
}

pub fn load(output_root: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(manifest_path(output_root)).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// 原子保存（临时文件 + rename）
pub fn save(output_root: &Path, m: &Manifest) -> Result<(), String> {
    let tmp = output_root.join(format!("{}.tmp", MANIFEST_NAME));
    let text = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("写清单失败: {}", e))?;
    fs::rename(&tmp, manifest_path(output_root)).map_err(|e| format!("保存清单失败: {}", e))
}

/// 绝对路径 → 相对输出根（正斜杠）
pub fn rel_key(output_root: &Path, abs: &Path) -> String {
    abs.strip_prefix(output_root)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn mtime_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 流式 SHA-256（128 KiB）+ 实际字节数
pub fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut f = fs::File::open(path).map_err(|e| format!("打开失败: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("读取失败: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, hex(&hasher.finalize())))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 字符串 SHA-256（十六进制小写）
pub fn hash_str(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex(&hasher.finalize())
}

/// 已记录且 size + mtime 未变 → 无需重新哈希
pub fn unchanged(m: &Manifest, archive: &str, size: u64, mtime: u64) -> bool {
    m.files
        .iter()
        .find(|f| f.archive == archive)
        .map(|f| f.size == size && f.mtime_ms == mtime)
        .unwrap_or(false)
}

/// 按 archive 覆盖或追加
pub fn upsert(m: &mut Manifest, f: ManifestFile) {
    if let Some(x) = m.files.iter_mut().find(|e| e.archive == f.archive) {
        *x = f;
    } else {
        m.files.push(f);
    }
    m.updated_at = now();
}

// ---------------------------------------------------------------- 校验

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyItem {
    pub rel_path: String,
    pub archive: String,
    /// ok | missing | size | hash | error
    pub status: String,
    pub expected_size: u64,
    pub actual_size: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VerifyReport {
    /// 清单内文件总数
    pub total: usize,
    pub ok: usize,
    pub missing: usize,
    pub size_bad: usize,
    pub hash_bad: usize,
    pub error: usize,
    /// 输出根内存在但未入清单的压缩包（rar/zip）
    pub unlisted: Vec<String>,
    /// 逐文件结果（清单顺序；取消时为已检部分）
    pub items: Vec<VerifyItem>,
}

/// 对账：清单记录 vs 磁盘现状；另扫描输出根找未入清单的压缩包。
/// `on_progress(index, total, archive_rel)` 每个清单文件回调一次。
pub fn verify(
    output_root: &Path,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(usize, usize, &str),
) -> Result<VerifyReport, String> {
    let m = load(output_root).ok_or_else(|| "尚无导出清单（请先执行一次导出）".to_string())?;
    let mut report = VerifyReport {
        total: m.files.len(),
        ..Default::default()
    };
    for (i, f) in m.files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        on_progress(i, m.files.len(), &f.archive);
        let path = output_root.join(&f.archive);
        let item = if !path.is_file() {
            report.missing += 1;
            VerifyItem {
                rel_path: f.rel_path.clone(),
                archive: f.archive.clone(),
                status: "missing".into(),
                expected_size: f.size,
                actual_size: 0,
            }
        } else {
            let actual = fs::metadata(&path).map(|x| x.len()).unwrap_or(0);
            if actual != f.size {
                report.size_bad += 1;
                VerifyItem {
                    rel_path: f.rel_path.clone(),
                    archive: f.archive.clone(),
                    status: "size".into(),
                    expected_size: f.size,
                    actual_size: actual,
                }
            } else {
                match hash_file(&path) {
                    Ok((_, sha)) if sha == f.sha256 => {
                        report.ok += 1;
                        VerifyItem {
                            rel_path: f.rel_path.clone(),
                            archive: f.archive.clone(),
                            status: "ok".into(),
                            expected_size: f.size,
                            actual_size: actual,
                        }
                    }
                    Ok((_, _)) => {
                        report.hash_bad += 1;
                        VerifyItem {
                            rel_path: f.rel_path.clone(),
                            archive: f.archive.clone(),
                            status: "hash".into(),
                            expected_size: f.size,
                            actual_size: actual,
                        }
                    }
                    Err(_) => {
                        report.error += 1;
                        VerifyItem {
                            rel_path: f.rel_path.clone(),
                            archive: f.archive.clone(),
                            status: "error".into(),
                            expected_size: f.size,
                            actual_size: actual,
                        }
                    }
                }
            }
        };
        report.items.push(item);
    }

    // 输出根内未入清单的压缩包（只看 rar/zip；说明文本等不计）
    let listed: HashSet<&str> = m.files.iter().map(|f| f.archive.as_str()).collect();
    let mut unlisted: Vec<String> = Vec::new();
    let mut stack = vec![output_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            let meta = match e.metadata() {
                Ok(x) => x,
                Err(_) => continue,
            };
            if crate::fsops::is_reparse(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if ext != "rar" && ext != "zip" {
                continue;
            }
            let rel = rel_key(output_root, &p);
            if rel != MANIFEST_NAME && !listed.contains(rel.as_str()) {
                unlisted.push(rel);
            }
        }
    }
    unlisted.sort();
    report.unlisted = unlisted;
    Ok(report)
}
