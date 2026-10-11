use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{Finding, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Sha256,
    Md5,
    Crc32,
}

impl HashKind {
    pub fn label(self) -> &'static str {
        match self {
            HashKind::Sha256 => "SHA-256",
            HashKind::Md5 => "MD5",
            HashKind::Crc32 => "CRC32",
        }
    }

    pub fn hex_len(self) -> usize {
        match self {
            HashKind::Sha256 => 64,
            HashKind::Md5 => 32,
            HashKind::Crc32 => 8,
        }
    }

    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "sha256" | "sha256sum" => Some(HashKind::Sha256),
            "md5" | "md5sum" => Some(HashKind::Md5),
            "sfv" | "crc32" => Some(HashKind::Crc32),
            _ => None,
        }
    }
}

/// 哈希清单中的一条记录
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// 解析后的目标文件路径
    pub path: PathBuf,
    pub kind: HashKind,
    /// 期望哈希（小写 hex）
    pub expected: String,
}

/// 文件旁的 sidecar 清单（如 a.zip.sha256）
#[derive(Debug, Clone)]
pub struct Sidecar {
    pub manifest: PathBuf,
    pub kind: HashKind,
    /// 期望哈希（小写 hex），None 表示清单中没有本文件的记录
    pub expected: Option<String>,
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn normalize_hex(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// 计算文件哈希（流式）
pub fn compute_hash(path: &Path, kind: HashKind) -> std::io::Result<String> {
    use sha2::Digest as _;

    let mut file = File::open(path)?;
    let mut buf = vec![0u8; 64 * 1024];

    match kind {
        HashKind::Sha256 => {
            let mut h = sha2::Sha256::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(hex_encode(&h.finalize()))
        }
        HashKind::Md5 => {
            let mut h = md5::Md5::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(hex_encode(&h.finalize()))
        }
        HashKind::Crc32 => {
            let mut h = crc32fast::Hasher::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(format!("{:08x}", h.finalize()))
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 解析哈希清单（.sha256 / .md5 / .sfv），路径相对于清单所在目录解析
pub fn parse_manifest(path: &Path) -> Result<Vec<ManifestEntry>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取清单 {}：{e}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));

    let mut entries = Vec::new();
    let mut seen = HashMap::new();

    for raw in content.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        // GNU 格式：`<hex>[ *]<文件名>`；SFV 格式：`<文件名> <crc32hex>`
        let mut parsed: Option<(HashKind, String, String)> = None;
        if let Some((first, rest)) = line.split_once(char::is_whitespace)
            && is_hex(first)
            && matches!(first.len(), 32 | 64)
        {
            let rest = rest.trim_start();
            let name = rest.strip_prefix('*').unwrap_or(rest).trim();
            if !name.is_empty() {
                let kind = if first.len() == 64 {
                    HashKind::Sha256
                } else {
                    HashKind::Md5
                };
                parsed = Some((kind, normalize_hex(first), name.to_string()));
            }
        }
        if parsed.is_none()
            && let Some((name, crc)) = line.rsplit_once(char::is_whitespace)
        {
            let crc = crc.trim();
            if crc.len() == 8 && is_hex(crc) {
                parsed = Some((HashKind::Crc32, normalize_hex(crc), name.trim().to_string()));
            }
        }
        let Some((kind, expected, name)) = parsed else {
            continue;
        };

        let target = if Path::new(&name).is_absolute() {
            PathBuf::from(&name)
        } else {
            base.join(&name)
        };
        if seen.insert(target.clone(), ()).is_none() {
            entries.push(ManifestEntry {
                path: target,
                kind,
                expected,
            });
        }
    }

    if entries.is_empty() {
        return Err(format!("清单中没有有效记录：{}", path.display()));
    }
    Ok(entries)
}

/// 建立「路径 → 期望哈希」索引，供批量扫描时查询
pub fn index_manifest(entries: &[ManifestEntry]) -> HashMap<PathBuf, (HashKind, String)> {
    entries
        .iter()
        .map(|e| (e.path.clone(), (e.kind, e.expected.clone())))
        .collect()
}

/// 自动寻找 sidecar：a.zip.sha256 / a.sha256 / a.zip.md5 / ...
pub fn find_sidecar(file: &Path) -> Option<Sidecar> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !name.is_empty() {
        for ext in ["sha256", "md5", "sfv"] {
            candidates.push(file.with_file_name(format!("{name}.{ext}")));
        }
    }
    for ext in ["sha256", "md5", "sfv"] {
        candidates.push(file.with_extension(ext));
    }

    candidates.into_iter().find_map(|c| {
        if c == file || !c.is_file() {
            return None;
        }
        parse_sidecar(file, &c)
    })
}

fn parse_sidecar(file: &Path, manifest: &Path) -> Option<Sidecar> {
    let ext = manifest.extension()?.to_str()?.to_ascii_lowercase();
    let kind = HashKind::from_ext(&ext)?;
    let content = std::fs::read_to_string(manifest).ok()?;
    let base = manifest.parent().unwrap_or_else(|| Path::new("."));

    if kind == HashKind::Crc32 {
        // SFV：查找本文件的记录
        for raw in content.lines() {
            let line = raw.trim_end_matches('\r').trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let (name, crc) = line.rsplit_once(char::is_whitespace)?;
            let crc = crc.trim();
            if crc.len() != 8 || !is_hex(crc) {
                continue;
            }
            let target = if Path::new(name).is_absolute() {
                PathBuf::from(name)
            } else {
                base.join(name.trim())
            };
            if same_file(&target, file) {
                return Some(Sidecar {
                    manifest: manifest.to_path_buf(),
                    kind,
                    expected: Some(normalize_hex(crc)),
                });
            }
        }
        return Some(Sidecar {
            manifest: manifest.to_path_buf(),
            kind,
            expected: None,
        });
    }

    // 单哈希格式：取第一个长度匹配的 hex token
    let expected = content
        .split_whitespace()
        .find(|t| t.len() == kind.hex_len() && is_hex(t))
        .map(normalize_hex);
    Some(Sidecar {
        manifest: manifest.to_path_buf(),
        kind,
        expected,
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.file_name(), b.file_name()) {
        (Some(x), Some(y)) => x
            .to_string_lossy()
            .eq_ignore_ascii_case(&y.to_string_lossy()),
        _ => false,
    }
}

/// 校验 sidecar 中记录的哈希
pub fn verify_sidecar(file: &Path, sidecar: &Sidecar) -> Vec<Finding> {
    match &sidecar.expected {
        Some(expected) => compare_hash(file, sidecar.kind, expected),
        None => vec![Finding::warning(
            "HASH_SIDECAR_MISS",
            format!(
                "sidecar 清单 {} 中没有本文件的记录",
                sidecar.manifest.display()
            ),
            None,
        )],
    }
}

/// 比对实际哈希与期望值
pub fn compare_hash(file: &Path, kind: HashKind, expected: &str) -> Vec<Finding> {
    let actual = match compute_hash(file, kind) {
        Ok(h) => h,
        Err(e) => {
            return vec![Finding::unknown(
                "HASH_READ_ERROR",
                format!("计算哈希失败：{e}"),
                None,
            )];
        }
    };
    let expected = normalize_hex(expected);
    if actual == expected {
        vec![Finding::ok(
            "HASH_OK",
            format!("{} 校验通过（{actual}）", kind.label()),
        )]
    } else {
        vec![Finding::corrupt(
            "HASH_MISMATCH",
            format!(
                "{} 不匹配：清单记录 {expected}，实际 {actual}",
                kind.label()
            ),
            None,
        )]
    }
}

/// 清单中的文件缺失
pub fn missing_entry_finding(path: &Path) -> Finding {
    Finding::new(
        Severity::Warning,
        "HASH_FILE_MISSING",
        format!("清单中的文件不存在：{}", path.display()),
        None,
    )
}
