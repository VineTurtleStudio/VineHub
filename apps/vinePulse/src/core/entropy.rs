use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// 采样大小：头 / 中段 / 尾部各取这么多字节
const SAMPLE: usize = 32 * 1024;

/// Shannon 熵（字节/字符），范围 0.0 ~ 8.0
pub fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// 从文件头 / 中段 / 尾部采样，返回采样字节
pub fn sample_file(path: &Path, size: u64) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    if size == 0 {
        return Ok(out);
    }
    let mut f = File::open(path)?;
    let n = SAMPLE as u64;

    // 头部
    let head_len = n.min(size);
    let mut buf = vec![0u8; head_len as usize];
    f.read_exact(&mut buf)?;
    out.extend_from_slice(&buf);

    // 中段
    if size > n * 2 {
        let mid = size / 2 - n / 2;
        let _ = f.seek(SeekFrom::Start(mid))?;
        let mut buf = vec![0u8; n as usize];
        let got = f.read(&mut buf)?;
        buf.truncate(got);
        out.extend_from_slice(&buf);
    }

    // 尾部
    if size > n {
        let _ = f.seek(SeekFrom::Start(size - n))?;
        let mut buf = vec![0u8; n as usize];
        let got = f.read(&mut buf)?;
        buf.truncate(got);
        out.extend_from_slice(&buf);
    }

    Ok(out)
}

/// 采样是否全部为 0（全零填充）
pub fn all_zero(sample: &[u8]) -> bool {
    !sample.is_empty() && sample.iter().all(|&b| b == 0)
}
